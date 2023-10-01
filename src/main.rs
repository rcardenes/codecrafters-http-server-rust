use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use nom::ExtendInto;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

type Response = Vec<Vec<u8>>;

#[derive(Clone)]
struct Route {
    path: PathBuf,
    exact: bool, // If true, the path must match `prefix` exactly
                 // Otherwise, this is a prefix
    handler: fn(Request) -> io::Result<Response>,
}

impl Route {
    fn new(path: &str, exact: bool, handler: fn(Request) -> io::Result<Response>) -> Self {
        Self {
            path: Path::new(path).to_path_buf(),
            exact,
            handler
        }
    }

    fn matches(&self, request: &Request) -> bool {
        if self.exact {
            self.path == request.path
        } else {
            request.path.starts_with(&self.path)
        }
    }
}

struct HeaderField {
    name: String,
    value: String,
}

struct Request {
    path: PathBuf,
    headers: Vec<HeaderField>,
    body: Option<Vec<u8>>,
}

impl Request {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            headers: vec![],
            body: None
        }
    }

    fn add_header(&mut self, name: &str, value: &str) {
        self.headers.push(HeaderField {
            name: name.to_string(),
            value: value.to_string()
        })
    }

    fn set_body(&mut self, content: Vec<u8>) {
        self.body = Some(content);
    }

    fn get_header(&self, needle: &str) -> Option<String> {
        for HeaderField { name, value } in &self.headers {
            if name == needle {
                return Some(value.to_string())
            }
        }
        None
    }

    fn content_length(&self) -> usize {
        self.get_header("Content-Length")
            .map(|value| value.parse::<usize>().unwrap())
            .or_else(|| Some(0usize))
            .unwrap()
    }
}

fn build_error<T>(kind: ErrorKind, msg: &str) -> io::Result<T> {
    Err(io::Error::new(kind, msg))
}

async fn parse_query(reader: &mut BufReader<TcpStream>) -> io::Result<Request> {
    let mut buf = String::new();
    reader.read_line(&mut buf).await?;
    let parts = buf.split_whitespace().collect::<Vec<_>>();
    let path = parts.get(1)
        .cloned()
        .map_or(
            build_error(
                ErrorKind::InvalidData,
                "Invalid message line. Expecting GET /path HTTP/1.1"
            ),
            |path| {
                Ok(PathBuf::from(path))
            }
        )?;

    let mut request = Request::new(path);

    buf.clear();
    while let Ok(size) = reader.read_line(&mut buf).await {
        if size == 0 {
            return build_error(
                ErrorKind::InvalidData,
                "Invalid query. Unexpected EOF"
            );
        } else if buf == "\r\n" {
            break
        } else {
            let trimmed = buf.trim_end();
            if let Some((name, value)) = trimmed.split_once(": ") {
                request.add_header(name, value);
            } else {
                return build_error(
                    ErrorKind::InvalidData,
                    &format!("Invalid header: {}", trimmed)
                )
            };
        }
        buf.clear();
    }

    /// Not going to read this yet. The naive solution of reading everything from this
    /// point could easily lead to a DoS

    // let content_length = request.content_length();
    // if content_length > 0 {
    //     buf.clear();
    //     reader.read_line(&mut buf).await?;
    //     if buf != "\r\n" {
    //         return build_error(
    //             ErrorKind::InvalidData,
    //             "End of response headers marker not found",
    //         );
    //     }
    // }

    Ok(request)
}

fn handle_echo(request: Request) -> io::Result<Response> {
    let mut buf: Response = vec![
        b"HTTP/1.1 200 OK\r\n".to_vec(),
        b"Content-Type: text/plain\r\n".to_vec()
    ];

    let mut text = request.path.as_os_str().as_bytes()[6..].to_vec();
    buf.push(
        format!("Content-Length: {}\r\n", text.len())
            .as_bytes()
            .to_vec()
    );
    buf.push(b"\r\n".to_vec());
    b"\r\n".extend_into(&mut text);
    buf.push(text);

    Ok(buf)
}

fn handle_user_agent(request: Request) -> io::Result<Response> {
    eprintln!("Handling user agent...");
    if let Some(agent) = request.get_header("User-Agent") {
        let mut buf: Response = vec![
            b"HTTP/1.1 200 OK\r\n".to_vec(),
            b"Content-Type: text/plain\r\n".to_vec(),
        ];

        buf.push(
            format!("Content-Length: {}\r\n\r\n", agent.len())
                .as_bytes()
                .to_vec()
        );
        let mut text = agent.into_bytes();
        b"\r\n".extend_into(&mut text);
        buf.push(text);

        Ok(buf)
    } else {
        build_error(
            ErrorKind::InvalidData,
            "Expected User-Agent header, but not found"
        )
    }
}

async fn handle_connection(stream: TcpStream, routes: &[Route]) -> io::Result<()> {
    let mut reader = BufReader::new(stream);

    let request = parse_query(&mut reader).await?;

    for route in routes {
        if route.matches(&request) {
            for block in (route.handler)(request)? {
                reader.write(&block).await?;
            }
            break;
        }
    }

    Ok(())
}

fn declare_routes() -> Vec<Route> {
    vec![
        Route::new("/", true,
                  |_| { Ok(vec![b"HTTP/1.1 200 OK\r\n\r\n".to_vec()]) }),
        Route::new("/echo/", false, handle_echo),
        Route::new("/user-agent", true, handle_user_agent),
        // The default, it matches anything
        Route::new("", false,
                  |_| { Ok(vec![b"HTTP/1.1 404 Not Found\r\n\r\n".to_vec()]) }),
    ]
}

const SERVER_ADDRESS: &str = "127.0.0.1:4221";

#[tokio::main]
async fn main() -> io::Result<()> {
    let listener = TcpListener::bind(SERVER_ADDRESS).await?;
    let routes = declare_routes();
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                eprintln!("Accepted connection from: {addr}");
                let cloned = routes.clone();
                tokio::spawn(async move {
                    handle_connection(stream, &cloned).await
                        .map_err(|error| {
                            eprintln!("Handling connection: {error}");
                            Ok::<_, io::Error>(())
                        }).unwrap();
                });
            }
            Err(error) => {
                eprintln!("Accepting incoming connection: {error}");
            }
        }
    }
}
