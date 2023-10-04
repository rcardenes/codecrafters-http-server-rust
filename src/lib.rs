use crate::config::Configuration;
use crate::request::Request;
use anyhow::Result;
use std::future::Future;
use std::io::ErrorKind;
use std::pin::Pin;
use tokio::io::{self, AsyncBufRead, AsyncWrite};

pub mod config;
pub mod handlers;
pub mod request;
pub mod response;
pub mod route;

// Custom types

pub type Reader<'a> = dyn AsyncBufRead + Unpin + Send + Sync + 'a;
pub type Writer = dyn AsyncWrite + Unpin + Send + Sync;

pub type HandlerReturn<'a> = Result<response::Response<'a>>;
pub type PinnedReturn<'a> = Pin<Box<dyn Future<Output = HandlerReturn<'a>> + Send + 'a>>;
pub type Handler = for<'a> fn(&'a Configuration, Request<'a>) -> PinnedReturn<'a>;

// Structs and enums

#[derive(Clone)]
struct HeaderField {
    name: String,
    value: String,
}

pub enum Payload<'a> {
    Simple(Vec<Vec<u8>>),
    ReadStream(Box<Reader<'a>>),
}

#[derive(Clone)]
pub enum StatusCode {
    HttpOk,
    Created,
    NotFound,
    Forbidden,
    InternalServerError,
}

#[derive(Clone, PartialEq)]
pub enum HttpVerb {
    Unknown,
    Any,
    Get,
    Post,
}

pub fn build_error<T>(kind: ErrorKind, msg: &str) -> Result<T> {
    Err(io::Error::new(kind, msg).into())
}
