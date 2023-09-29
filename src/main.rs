use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
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

async fn handle_connection(stream: TcpStream) -> io::Result<()> {
    let mut reader = BufReader::new(stream);

    let request = parse_query(&mut reader).await?;
    let path = request.path.as_os_str().as_bytes();

    let response: &[u8] = if path == b"/" {
        b"HTTP/1.1 200 OK\r\n\r\n"
    } else {
        b"HTTP/1.1 404 Not Found\r\n\r\n"
    };
    reader.write(response).await?;

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
