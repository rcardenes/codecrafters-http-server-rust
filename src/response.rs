use crate::{HeaderField, Payload, StatusCode};
use anyhow::Result;
use tokio::io::AsyncWriteExt;
use tokio::net::tcp::WriteHalf;

pub struct Response<'a> {
    code: StatusCode,
    headers: Vec<HeaderField>,
    payload: Option<Payload<'a>>,
}

impl<'a> Response<'a> {
    pub fn from_status(status: StatusCode) -> Self {
        Self {
            code: status,
            headers: vec![],
            payload: None,
        }
    }

    pub fn ok(content: Payload<'a>) -> Self {
        Self {
            code: StatusCode::HttpOk,
            headers: vec![],
            payload: Some(content),
        }
    }

    pub fn not_found() -> Self {
        Response::from_status(StatusCode::NotFound)
    }

    pub fn forbidden() -> Self {
        Response::from_status(StatusCode::Forbidden)
    }

    pub fn internal_error() -> Self {
        Response::from_status(StatusCode::InternalServerError)
    }

    pub fn payload(&mut self) -> Option<Payload> {
        self.payload.take()
    }
    pub fn add_header(&mut self, name: &str, value: &str) {
        self.headers.push(HeaderField {
            name: name.to_string(),
            value: value.to_string(),
        })
    }

    pub async fn write_header<'b>(&self, stream: &mut WriteHalf<'b>) -> Result<()> {
        let (code, msg) = match self.code {
            StatusCode::HttpOk => (200, "OK"),
            StatusCode::Created => (201, "Created"),
            StatusCode::NotFound => (404, "Not Found"),
            StatusCode::Forbidden => (403, "Forbidden"),
            StatusCode::InternalServerError => (500, "Internal Server Error"),
        };
        let status_line = format!("HTTP/1.1 {} {}\r\n", code, msg);
        stream.write_all(status_line.as_bytes()).await?;
        for header in self.headers.iter() {
            let output = format!("{}: {}\r\n", header.name, header.value);
            stream.write_all(output.as_bytes()).await?;
        }

        // End of header
        stream.write_all(b"\r\n").await?;
        stream.flush().await?;
        Ok(())
    }
}
