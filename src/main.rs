use std::io::ErrorKind;
use std::path::PathBuf;
use anyhow::Result;
use tokio::{
    io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{
        TcpListener,
        TcpStream,
    }
};

use http_server_starter_rust::{HttpVerb, Payload, Reader, StatusCode, config::Configuration, request::Request, response::Response, route::{Route, RouteTarget}, build_error};
use http_server_starter_rust::handlers::{handle_download_file, handle_echo, handle_upload_file, handle_user_agent};

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

async fn handle_connection(config: &Configuration, mut stream: TcpStream, routes: &[Route]) -> Result<()> {
    let (read_half, mut writer) = stream.split();
    let reader = BufReader::new(read_half);

    let request = parse_query(Box::new(reader)).await?;

    for route in routes {
        if let Some(size) = route.matches(&request) {
            let mut response = route.handle(
                config,
                Request::strip_path_prefix(request, size)
            ).await?;

            response.write_header(&mut writer).await?;
            if let Some(payload) = response.payload() {
                match payload {
                    Payload::Simple(response) => {
                        for block in response {
                            writer.write(&block).await?;
                        }
                    }
                    Payload::ReadStream(mut stream) => {
                        io::copy_buf(&mut stream, &mut writer).await?;
                    }
                }
            }
            return Ok(())
        }
    }

    // Default: 404
    Response::from_status(StatusCode::NotFound)
        .write_header(&mut writer).await
}

fn declare_routes() -> Vec<Route> {
    vec![
        Route::new(HttpVerb::Get, "/", true, RouteTarget::Static(StatusCode::HttpOk)),
        Route::new(HttpVerb::Get, "/echo/", false, RouteTarget::Dynamic(handle_echo)),
        Route::new(HttpVerb::Get,"/user-agent", true, RouteTarget::Dynamic(handle_user_agent)),
        Route::new(HttpVerb::Get,"/files/", false, RouteTarget::Dynamic(handle_download_file)),
        Route::new(HttpVerb::Post, "/files/", false, RouteTarget::Dynamic(handle_upload_file)),
    ]
}

const SERVER_ADDRESS: &str = "127.0.0.1:4221";

#[tokio::main]
async fn main() -> Result<()> {
    let config = Configuration::get();
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
