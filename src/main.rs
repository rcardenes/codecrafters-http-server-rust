use std::env;
use std::ffi::OsStr;
use std::future::Future;
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::pin::Pin;
use anyhow::Result;
use tokio::{fs::File, io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader}, net::{TcpListener, TcpStream}, pin};
use tokio::io::AsyncBufRead;

#[derive(Clone)]
struct HeaderField {
    name: String,
    value: String,
}

#[derive(Clone)]
enum StatusCode {
    HttpOk,
    NotFound,
    Forbidden,
    InternalServerError,
}

enum Payload {
    Simple(Vec<Vec<u8>>),
    Stream(Box<dyn AsyncBufRead + Unpin + Send + Sync>)
}

struct Response {
    code: StatusCode,
    headers: Vec<HeaderField>,
    payload: Option<Payload>,
}

impl Response {
    fn from_status(status: StatusCode) -> Self {
        Self {
            code: status,
            headers: vec![],
            payload: None
        }
    }

    fn ok(content: Payload) -> Self {
        Self {
            code: StatusCode::HttpOk,
            headers: vec![],
            payload: Some(content)
        }
    }

    fn not_found() -> Self { Response::from_status(StatusCode::NotFound) }

    fn forbidden() -> Self { Response::from_status(StatusCode::Forbidden) }

    fn internal_error() -> Self { Response::from_status(StatusCode::InternalServerError) }

    fn add_header(&mut self, name: &str, value: &str) {
        self.headers.push(HeaderField {
            name: name.to_string(),
            value: value.to_string()
        })
    }

    async fn write_header(&self, stream: &mut BufReader<TcpStream>) -> Result<()> {
        let (code, msg) = match self.code {
            StatusCode::HttpOk => (200, "OK"),
            StatusCode::NotFound => (404, "Not Found"),
            StatusCode::Forbidden => (403, "Forbidden"),
            StatusCode::InternalServerError => (500, "Internal Server Error"),
        };
        let status_line = format!("HTTP/1.1 {} {}\r\n", code, msg);
        stream.write(status_line.as_bytes()).await?;
        for header in self.headers.iter() {
            let output = format!("{}: {}\r\n", header.name, header.value);
            stream.write(output.as_bytes()).await?;
        }

        // End of header
        stream.write(b"\r\n").await?;
        stream.flush().await?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct Configuration {
    root_dir: Option<PathBuf>,
}

type HandlerReturn = Result<Response>;
type PinnedReturn<'a> = Pin<Box<dyn Future<Output=HandlerReturn> + Send + 'a>>;
type Handler = fn(&Configuration, Request) -> PinnedReturn;

#[derive(Clone)]
struct Route
{
    path: PathBuf,
    exact: bool, // If true, the path must match `prefix` exactly
                 // Otherwise, this is a prefix
    handler: RouteTarget,
}

#[derive(Clone)]
enum RouteTarget {
    Static(StatusCode),
    Dynamic(Handler),
}

impl Into<RouteTarget> for Handler {
    fn into(self) -> RouteTarget {
        RouteTarget::Dynamic(self)
    }
}

impl RouteTarget {
    async fn invoke(&self, config: &Configuration, request: Request) -> Result<Response> {
        match self {
            RouteTarget::Static(code) => {
                Ok(Response::from_status(code.clone()))
            },
            RouteTarget::Dynamic(handler) => {
                (handler)(config, request).await
            },
        }
    }
}

impl Route {
    fn new(path: &str, exact: bool, handler: RouteTarget) -> Self {
        Self {
            path: PathBuf::from(path),
            exact,
            handler
        }
    }

    fn matches(&self, request: &Request) -> Option<usize> {
        let does_match = if self.exact {
            self.path == request.path
        } else {
            request.path.starts_with(&self.path)
        };

        if does_match {
            Some(self.path.as_os_str().len())
        } else {
            None
        }
    }
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

    fn get_header(&self, needle: &str) -> Option<String> {
        for HeaderField { name, value } in &self.headers {
            if name == needle {
                return Some(value.to_string())
            }
        }
        None
    }

    // fn content_length(&self) -> usize {
    //     self.get_header("Content-Length")
    //         .map(|value| value.parse::<usize>().unwrap())
    //         .or_else(|| Some(0usize))
    //         .unwrap()
    // }
    //
    fn strip_path_prefix(req: Request, pref_length: usize) -> Self {
        let parts = req.path
            .as_os_str()
            .as_bytes()
            .split_at(pref_length);
        Self {
            path: PathBuf::from(OsStr::from_bytes(parts.1)),
            headers: req.headers,
            body: req.body,
        }
    }
}

fn build_error<T>(kind: ErrorKind, msg: &str) -> Result<T> {
    Err(io::Error::new(kind, msg).into())
}

async fn parse_query(reader: &mut BufReader<TcpStream>) -> Result<Request> {
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

    // Not going to read this yet. The naive solution of reading everything from this
    // point could easily lead to a DoS

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

fn handle_echo(_config: &Configuration, request: Request) -> PinnedReturn {
    Box::pin(async move {
        let text = request.path.as_os_str().as_bytes().to_vec();
        let length = text.len().to_string();

        let mut response = Response::ok(Payload::Simple(vec![text]));
        response.add_header("Content-Type", "text/plain");
        response.add_header("Content-Length", &length);

        Ok(response)
    })
}

fn handle_user_agent(_config: &Configuration, request: Request) -> PinnedReturn {
    Box::pin(async move {
        if let Some(agent) = request.get_header("User-Agent") {
            let length = agent.len().to_string();
            let mut response = Response::ok(Payload::Simple(vec![agent.into_bytes()]));
            response.add_header("Content-Type", "text/plain");
            response.add_header("Content-Length", &length);

            Ok(response)
        } else {
            build_error(
                ErrorKind::InvalidData,
                "Expected User-Agent header, but not found",
            )
        }
    })
}

fn handle_files(config: &Configuration, request: Request) -> PinnedReturn {
    Box::pin(async move {
        let mut full_path = match &config.root_dir {
            Some(base_dir) => base_dir.clone(),
            None => env::current_dir()?,
        };
        full_path.push(request.path);

        match File::open(full_path).await {
            Ok(file) => {
                let size = file.metadata().await?.len();
                let mut response = Response::ok(
                    Payload::Stream(Box::new(BufReader::new(file)))
                );
                response.add_header("Content-Length", &size.to_string());
                response.add_header("Content-Type", "application/octet-stream");
                response.add_header("Content-Disposition", "attachment");
                Ok(response)
            }
            Err(error) => match error.kind() {
                ErrorKind::NotFound => Ok(Response::not_found()),
                ErrorKind::PermissionDenied => Ok(Response::forbidden()),
                _ => Ok(Response::internal_error()),
            }
        }
    })
}

async fn handle_connection(config: &Configuration, stream: TcpStream, routes: &[Route]) -> Result<()> {
    let mut reader = BufReader::new(stream);

    let request = parse_query(&mut reader).await?;

    for route in routes {
        if let Some(size) = route.matches(&request) {
            let response = route.handler.invoke(
                config,
                Request::strip_path_prefix(request, size)
            ).await?;

            response.write_header(&mut reader).await?;
            if let Some(payload) = response.payload {
                match payload {
                    Payload::Simple(response) => {
                        for block in response {
                            reader.write(&block).await?;
                        }
                    }
                    Payload::Stream(stream) => {
                        let mut writer = reader;
                        pin!(stream);
                        io::copy_buf(&mut stream, &mut writer).await?;
                    }
                }
            }
            break;
        }
    }

    Ok(())
}

fn declare_routes() -> Vec<Route> {
    vec![
        Route::new("/", true, RouteTarget::Static(StatusCode::HttpOk)),
        Route::new("/echo/", false, RouteTarget::Dynamic(handle_echo)),
        Route::new("/user-agent", true, RouteTarget::Dynamic(handle_user_agent)),
        Route::new("/files/", false, RouteTarget::Dynamic(handle_files)),
        // The default, it matches anything
        Route::new("", false, RouteTarget::Static(StatusCode::NotFound)),
    ]
}

fn get_configuration() -> Configuration {
    let mut directory: Option<PathBuf> = None;
    let args: Vec<String> = env::args().collect();

    if args.get(1) == Some(&"--directory".to_string()) {
        if let Some(path) = args.get(2) {
            directory = Some(PathBuf::from(path));
        }
    }

    Configuration {
        root_dir: directory,
    }
}

const SERVER_ADDRESS: &str = "127.0.0.1:4221";

#[tokio::main]
async fn main() -> Result<()> {
    let config = get_configuration();
    let listener = TcpListener::bind(SERVER_ADDRESS).await?;
    let routes = declare_routes();

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                eprintln!("Accepted connection from: {addr}");
                let config = config.clone();
                let cloned = routes.clone();
                tokio::spawn(async move {
                    handle_connection(&config, stream, &cloned).await
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
