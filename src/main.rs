use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use nom::ExtendInto;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

struct Request {
    path: PathBuf,
}

async fn parse_query(reader: &mut BufReader<TcpStream>) -> io::Result<Request> {
    let mut buf = String::new();
    reader.read_line(&mut buf).await?;
    let parts = buf.split_whitespace().collect::<Vec<_>>();
    parts.get(1)
        .cloned()
        .map_or(Err(io::Error::new(
            ErrorKind::InvalidInput,
            "Invalid header. Expecting GET /path HTTP/1.1"
        )),
                |path| Ok(Request {
                    path: PathBuf::from(path)
                })
        )
}

fn handle_echo(request: Request) -> Vec<Vec<u8>> {
    let mut buf: Vec<Vec<u8>> = vec![
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

    buf
}

async fn handle_connection(stream: TcpStream) -> io::Result<()> {
    let mut reader = BufReader::new(stream);

    let request = parse_query(&mut reader).await?;
    let path = request.path.as_os_str().as_bytes();

    let response: Vec<Vec<u8>> = if path == b"/" {
        vec![b"HTTP/1.1 200 OK\r\n\r\n".to_vec()]
    } else if path.starts_with(b"/echo/") {
        handle_echo(request)
    }
    else {
        vec![b"HTTP/1.1 404 Not Found\r\n\r\n".to_vec()]
    };
    for block in response {
        reader.write(&block).await?;
    }

    Ok(())
}

const SERVER_ADDRESS: &str = "127.0.0.1:4221";

#[tokio::main]
async fn main() -> io::Result<()> {
    let listener = TcpListener::bind(SERVER_ADDRESS).await?;

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                eprintln!("Accepted connection from: {addr}");
                tokio::spawn(async move {
                    handle_connection(stream).await
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
