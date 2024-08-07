use crate::{
    build_error,
    config::Configuration,
    request::Request,
    response::Response,
    {Payload, PinnedReturn, Reader, StatusCode, Writer},
};
use anyhow::Result;
use async_compression::tokio::bufread::GzipEncoder;
use std::io::{ErrorKind, Cursor};
use std::os::unix::ffi::OsStrExt;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};

pub fn handle_echo<'a>(_config: &Configuration, request: Request<'a>) -> PinnedReturn<'a> {
    Box::pin(async move {
        let raw_text = request.path().as_os_str().as_bytes().to_vec();
        let text = if request.wants_gzip_encoding() {
            let mut buf = vec![];
            let _ = GzipEncoder::new(Cursor::new(raw_text)).read_to_end(&mut buf).await;
            buf
        } else {
            raw_text
        };
        let tlen = text.len();
        let length = tlen.to_string();
        let payload = Payload::Simple(vec![text]);

        let mut response = Response::ok(payload);
        response.add_header("Content-Type", "text/plain");

        if tlen > 0 {
            response.add_header("Content-Length", &length);
        }

        Ok(response)
    })
}

pub fn handle_user_agent<'a>(_config: &Configuration, request: Request<'a>) -> PinnedReturn<'a> {
    Box::pin(async move {
        if let Some(agent) = request.get_header("User-Agent") {
            let length = agent.len().to_string();
            let mut response = Response::ok(Payload::Simple(vec![agent.as_bytes().to_owned()]));
            response.add_header("Content-Type", "text/plain");
            if agent.len() > 0 {
                response.add_header("Content-Length", &length);
            }

            Ok(response)
        } else {
            build_error(
                ErrorKind::InvalidData,
                "Expected User-Agent header, but not found",
            )
        }
    })
}

pub fn handle_download_file<'a>(
    config: &'a Configuration,
    request: Request<'a>,
) -> PinnedReturn<'a> {
    Box::pin(async move {
        let full_path = config.resolve_path(request.path())?;

        match File::open(full_path).await {
            Ok(file) => {
                let size = file.metadata().await?.len();
                let buf_reader = BufReader::new(file);
                let mut response =
                    Response::ok(Payload::ReadStream(Box::new(buf_reader)));
                if size > 0 {
                    response.add_header("Content-Length", &size.to_string());
                }
                response.add_header("Content-Type", "application/octet-stream");
                response.add_header("Content-Disposition", "attachment");
                Ok(response)
            }
            Err(error) => match error.kind() {
                ErrorKind::NotFound => Ok(Response::not_found()),
                ErrorKind::PermissionDenied => Ok(Response::forbidden()),
                _ => Ok(Response::internal_error()),
            },
        }
    })
}

const COPY_BUFFER_DEFAULT_SIZE: usize = 1024;

async fn copy_bytes<'a>(
    reader: &mut Reader<'a>,
    writer: &mut Writer,
    len: usize,
    buf_size: usize,
) -> Result<usize> {
    let mut remaining = len;

    while remaining > 0 {
        let mut buffer = vec![0; std::cmp::min(buf_size, remaining)];
        remaining -= reader.read_exact(&mut buffer).await?;
        writer.write_all(&buffer).await?;
    }
    writer.flush().await?;

    Ok(len - remaining)
}

pub fn handle_upload_file<'a>(
    config: &'a Configuration,
    mut request: Request<'a>,
) -> PinnedReturn<'a> {
    Box::pin(async move {
        let full_path = config.resolve_path(request.path())?;

        match File::create(full_path).await {
            Ok(mut file) => {
                if let (Some(length), Some(Payload::ReadStream(mut reader))) =
                    (request.content_length(), request.body())
                {
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
            },
        }
    })
}
