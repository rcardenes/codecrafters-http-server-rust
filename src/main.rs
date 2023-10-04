use std::env;
use std::ffi::OsStr;
use std::future::Future;
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::pin::Pin;
use anyhow::Result;
use tokio::{
    fs::File,
    io::{self, AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::{
        tcp::WriteHalf,
        TcpListener,
        TcpStream,
    }
};
use tokio::io::AsyncReadExt;

#[derive(Clone)]
struct HeaderField {
    name: String,
    value: String,
}

#[derive(Clone)]
enum StatusCode {
    HttpOk,
    Created,
    NotFound,
    Forbidden,
    InternalServerError,
}

type Reader<'a> = dyn AsyncBufRead + Unpin + Send + Sync + 'a;
type Writer = dyn AsyncWrite + Unpin + Send + Sync;

enum Payload<'a> {
    Simple(Vec<Vec<u8>>),
    ReadStream(Box<Reader<'a>>),
}

struct Response<'a> {
    code: StatusCode,
    headers: Vec<HeaderField>,
    payload: Option<Payload<'a>>,
}

impl<'a> Response<'a> {
    fn from_status(status: StatusCode) -> Self {
        Self {
            code: status,
            headers: vec![],
            payload: None
        }
    }

    fn ok(content: Payload<'a>) -> Self {
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

    async fn write_header<'b>(&self, stream: &mut WriteHalf<'b>) -> Result<()> {
        let (code, msg) = match self.code {
            StatusCode::HttpOk => (200, "OK"),
            StatusCode::Created => (201, "Created"),
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

type HandlerReturn<'a> = Result<Response<'a>>;
type PinnedReturn<'a> = Pin<Box<dyn Future<Output=HandlerReturn<'a>> + Send + 'a>>;
type Handler = for<'a> fn(&'a Configuration, Request<'a>) -> PinnedReturn<'a>;

#[derive(Clone, PartialEq)]
enum HttpVerb {
    Unknown,
    Any,
    Get,
    Post,
}

#[derive(Clone)]
struct Route
{
    verb: HttpVerb,
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
    async fn invoke<'a>(&'a self, config: &'a Configuration, request: Request<'a>) -> Result<Response> {
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
    fn new(verb: HttpVerb, path: &str, exact: bool, handler: RouteTarget) -> Self {
        Self {
            verb,
            path: PathBuf::from(path),
            exact,
            handler
        }
    }

    fn matches(&self, request: &Request) -> Option<usize> {
        let verb_matches = request.verb == HttpVerb::Any || request.verb == self.verb;
        let path_matches = if self.exact {
            self.path == request.path
        } else {
            request.path.starts_with(&self.path)
        };

        if verb_matches && path_matches {
            Some(self.path.as_os_str().len())
        } else {
            None
        }
    }
}

struct Request<'a> {
    verb: HttpVerb,
    path: PathBuf,
    headers: Vec<HeaderField>,
    body: Option<Payload<'a>>,
}

impl<'a> Request<'a> {
    fn new(verb: HttpVerb, path: PathBuf) -> Self {
        Self {
            verb,
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

    fn set_payload(&mut self, payload: Payload<'a>) {
        self.body = Some(payload)
    }

    fn content_length(&self) -> Option<usize> {
        self.get_header("Content-Length")
            .map(|value| value.parse::<usize>().unwrap())
    }

    fn strip_path_prefix(req: Request<'a>, pref_length: usize) -> Self {
        let parts = req.path
            .as_os_str()
            .as_bytes()
            .split_at(pref_length);
        Self {
            verb: req.verb,
            path: PathBuf::from(OsStr::from_bytes(parts.1)),
            headers: req.headers,
            body: req.body,
        }
    }
}

fn build_error<T>(kind: ErrorKind, msg: &str) -> Result<T> {
    Err(io::Error::new(kind, msg).into())
}

async fn parse_query<'a>(mut reader: Box<Reader<'a>>) -> Result<Request<'a>>
{
    let mut buf = String::new();
    reader.read_line(&mut buf).await?;
    let parts = buf.split_whitespace().collect::<Vec<_>>();
    let verb = match parts.get(0) {
        Some(&"GET") => HttpVerb::Get,
        Some(&"POST") => HttpVerb::Post,
        _ => HttpVerb::Unknown,
    };
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

    let mut request = Request::new(verb, path);

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

    request.set_payload(Payload::ReadStream(reader));

    Ok(request)
}

fn handle_echo<'a>(_config: &Configuration, request: Request<'a>) -> PinnedReturn<'a> {
    Box::pin(async move {
        let text = request.path.as_os_str().as_bytes().to_vec();
        let length = text.len().to_string();

        let mut response = Response::ok(Payload::Simple(vec![text]));
        response.add_header("Content-Type", "text/plain");
        response.add_header("Content-Length", &length);

        Ok(response)
    })
}

fn handle_user_agent<'a>(_config: &Configuration, request: Request<'a>) -> PinnedReturn<'a> {
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

fn handle_download_file<'a>(config: &'a Configuration, request: Request<'a>) -> PinnedReturn<'a> {
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
                    Payload::ReadStream(Box::new(BufReader::new(file)))
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

const COPY_BUFFER_DEFAULT_SIZE: usize = 1024;

async fn copy_bytes<'a>(reader: &mut Reader<'a>, writer: &mut Writer, len: usize, buf_size: usize) -> Result<usize> {
    let mut remaining = len;

    while remaining > 0 {
        let mut buffer = vec![0; std::cmp::min(buf_size, remaining)];
        remaining -= reader.read_exact(&mut buffer).await?;
        writer.write(&buffer).await?;
    }
    writer.flush().await?;

    Ok(len - remaining)
}

fn handle_upload_file<'a>(config: &'a Configuration, request: Request<'a>) -> PinnedReturn<'a> {
    Box::pin (async move {
        let mut full_path = match &config.root_dir {
            Some(base_dir) => base_dir.clone(),
            None => env::current_dir()?,
        };
        full_path.push(&request.path);

        match File::create(full_path).await {
            Ok(mut file) => {
                if let (Some(length), Some(Payload::ReadStream(mut reader))) = (request.content_length(), request.body) {
                    // TODO: Should probably check the actual read size
                    copy_bytes(&mut reader, &mut file, length, COPY_BUFFER_DEFAULT_SIZE).await?;
                    Ok(Response::from_status(StatusCode::Created))
                } else {
                    Ok(Response::internal_error())
                }
            }
            Err(error) => match error.kind() {
                ErrorKind::NotFound => Ok(Response::not_found()),
                ErrorKind::PermissionDenied => Ok(Response::forbidden()),
                _ => Ok(Response::internal_error()),
            }
        }
    })
}

async fn handle_connection(config: &Configuration, mut stream: TcpStream, routes: &[Route]) -> Result<()> {
    let (read, mut write) = stream.split();
    let reader = BufReader::new(read);

    let request = parse_query(Box::new(reader)).await?;

    for route in routes {
        if let Some(size) = route.matches(&request) {
            let response = route.handler.invoke(
                config,
                Request::strip_path_prefix(request, size)
            ).await?;

            response.write_header(&mut write).await?;
            if let Some(payload) = response.payload {
                match payload {
                    Payload::Simple(response) => {
                        for block in response {
                            write.write(&block).await?;
                        }
                    }
                    Payload::ReadStream(mut stream) => {
                        io::copy_buf(&mut stream, &mut write).await?;
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
        Route::new(HttpVerb::Get, "/", true, RouteTarget::Static(StatusCode::HttpOk)),
        Route::new(HttpVerb::Get, "/echo/", false, RouteTarget::Dynamic(handle_echo)),
        Route::new(HttpVerb::Get,"/user-agent", true, RouteTarget::Dynamic(handle_user_agent)),
        Route::new(HttpVerb::Get,"/files/", false, RouteTarget::Dynamic(handle_download_file)),
        Route::new(HttpVerb::Post, "/files/", false, RouteTarget::Dynamic(handle_upload_file)),
        // The default, it matches anything
        Route::new(HttpVerb::Any,"", false, RouteTarget::Static(StatusCode::NotFound)),
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
