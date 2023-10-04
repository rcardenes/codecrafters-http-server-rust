use crate::{HeaderField, HttpVerb, Payload};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub struct Request<'a> {
    verb: HttpVerb,
    path: PathBuf,
    headers: Vec<HeaderField>,
    body: Option<Payload<'a>>,
}

impl<'a> Request<'a> {
    pub fn new(verb: HttpVerb, path: PathBuf) -> Self {
        Self {
            verb,
            path,
            headers: vec![],
            body: None,
        }
    }

    pub fn verb(&'a self) -> &'a HttpVerb {
        &self.verb
    }

    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn body(&mut self) -> Option<Payload<'a>> {
        self.body.take()
    }

    pub fn add_header(&mut self, name: &str, value: &str) {
        self.headers.push(HeaderField {
            name: name.to_string(),
            value: value.to_string(),
        })
    }

    pub fn get_header(&self, needle: &str) -> Option<String> {
        for HeaderField { name, value } in &self.headers {
            if name == needle {
                return Some(value.to_string());
            }
        }
        None
    }

    pub fn set_payload(&mut self, payload: Payload<'a>) {
        self.body = Some(payload)
    }

    pub fn content_length(&self) -> Option<usize> {
        self.get_header("Content-Length")
            .map(|value| value.parse::<usize>().unwrap())
    }

    pub fn strip_path_prefix(req: Request<'a>, pref_length: usize) -> Self {
        let parts = req.path.as_os_str().as_bytes().split_at(pref_length);
        Self {
            verb: req.verb,
            path: PathBuf::from(OsStr::from_bytes(parts.1)),
            headers: req.headers,
            body: req.body,
        }
    }
}
