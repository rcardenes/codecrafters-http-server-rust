use anyhow::Result;
use std::path::PathBuf;
use crate::{Handler, HttpVerb, StatusCode};
use crate::config::Configuration;
use crate::request::Request;
use crate::response::Response;

#[derive(Clone)]
pub struct Route
{
    verb: HttpVerb,
    path: PathBuf,
    // If `exact` is true, the path must match `prefix` exactly
    // Otherwise, this is a prefix
    exact: bool,
    handler: RouteTarget,
}

#[derive(Clone)]
pub enum RouteTarget {
    Static(StatusCode),
    Dynamic(Handler),
}

impl Into<RouteTarget> for Handler {
    fn into(self) -> RouteTarget {
        RouteTarget::Dynamic(self)
    }
}

impl RouteTarget {
    pub async fn invoke<'a>(&'a self, config: &'a Configuration, request: Request<'a>) -> Result<Response> {
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
    pub fn new(verb: HttpVerb, path: &str, exact: bool, handler: RouteTarget) -> Self {
        Self {
            verb,
            path: PathBuf::from(path),
            exact,
            handler
        }
    }

    pub fn matches(&self, request: &Request) -> Option<usize> {
        let verb_matches = self.verb == HttpVerb::Any || request.verb() == &self.verb;
        let path_matches = if self.exact {
            self.path == request.path()
        } else {
            request.path().starts_with(&self.path)
        };

        if verb_matches && path_matches {
            Some(self.path.as_os_str().len())
        } else {
            None
        }
    }

    pub async fn handle<'a>(&'a self, config: &'a Configuration, request: Request<'a>) -> Result<Response> {
        self.handler.invoke(config, request).await
    }
}