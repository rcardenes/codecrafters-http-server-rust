use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

async fn handle_connection(stream: TcpStream) -> io::Result<()> {
    let mut reader = BufReader::new(stream);
    let mut buf = String::new();

    // Read header
    reader.read_line(&mut buf).await?;
    reader.write(b"HTTP/1.1 200 OK\r\n\r\n").await?;

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
