use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

fn main() {
    let listener = TcpListener::bind("127.0.0.1:4221").unwrap();

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut buf = String::new();
                reader.read_line(&mut buf).unwrap();
                stream.write(b"HTTP/1.1 200 OK\r\n\r\n").unwrap();
            }
            Err(e) => {
                println!("error: {}", e);
            }
        }
    }
}
