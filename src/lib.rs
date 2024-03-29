pub mod images;
pub mod items;
pub mod pages;
pub mod threads;
pub mod users;

use std::{any::Any, collections::HashMap};

use axum::{
    async_trait,
    body::{Body, Bytes},
    extract::{
        multipart::{MultipartError, MultipartRejection},
        FromRequest, Multipart,
    },
    handler::Handler,
    http::Request,
    response::{IntoResponse, Response},
    Router,
};
use derive_more::Display;
use marche_proc_macros::ErrorCode;
use serde::{de::DeserializeOwned, Serialize};
use thiserror::Error;

pub const DATE_FMT: &str = "%B %-d, %Y at %I:%M %P";

pub struct Endpoint {
    route_type: RouteType,
    path:       &'static str,
    handler:    &'static (dyn Any + Send + Sync + 'static),
    installer:
        fn(RouteType, &'static str, &'static (dyn Any + Send + Sync + 'static), Router) -> Router,
}

impl Endpoint {
    pub const fn new<I, A>(route_type: RouteType, path: &'static str, handler: &'static I) -> Self
    where
        I: Handler<A, (), Body> + Copy + Any + Send + Sync + 'static,
        A: 'static,
    {
        Self {
            path,
            route_type,
            handler: handler as &(dyn Any + Send + Sync + 'static),
            installer: install::<I, A>,
        }
    }

    pub fn install(&self, router: Router) -> Router {
        (self.installer)(self.route_type, self.path, self.handler, router)
    }
}

inventory::collect!(Endpoint);

#[derive(Copy, Clone, Display)]
pub enum RouteType {
    #[display(fmt = "GET")]
    Get,
    #[display(fmt = "POST")]
    Post,
}

pub fn install<I, A>(
    route_type: RouteType,
    path: &'static str,
    handler: &'static (dyn Any + Send + Sync + 'static),
    router: Router,
) -> Router
where
    I: Handler<A, ()> + Copy,
    A: 'static,
{
    tracing::info!("{route_type} {path} registered");
    router.route(
        &path,
        match route_type {
            RouteType::Get => axum::routing::get(*handler.downcast_ref::<I>().unwrap()),
            RouteType::Post => axum::routing::post(*handler.downcast_ref::<I>().unwrap()),
        },
    )
}

#[macro_export]
macro_rules! get {
    ( $suffix:literal, $func:item ) => {
        inventory::submit! {
            crate::Endpoint::new::<_, _>(
                crate::RouteType::Get, $suffix, &marche_proc_macros::get_fn_name!( $func )
            )
        }
        $func
    };
}

#[macro_export]
macro_rules! post {
    ( $suffix:literal, $func:item ) => {
        inventory::submit! {
            crate::Endpoint::new::<_, _>(
                crate::RouteType::Post, $suffix, &marche_proc_macros::get_fn_name!( $func )
            )
        }
        $func
    };
}

/// An error type must give a proper status code for error handling.
pub trait ErrorCode {
    fn error_code(&self) -> http::StatusCode;
}

/// A multipart form that includes a file (which must be named "file").
/// Ideally we'd like this to be
#[derive(Debug)]
pub struct MultipartForm<Form, const N: u64> {
    pub form: Form,
    pub file: Option<File>,
}

#[derive(Debug)]
pub struct File {
    pub name:  String,
    pub bytes: Bytes,
}

#[derive(Debug, Serialize, Error, ErrorCode)]
pub enum MultipartFormError {
    #[error("invalid content length")]
    InvalidContentLength,
    #[error("invalid field")]
    InvalidField,
    #[error("error parsing form: {0}")]
    ParseError(
        #[from]
        #[serde(skip)]
        serde_json::Error,
    ),
    #[error("multipart rejection: {0}")]
    MultipartRejection(
        #[from]
        #[serde(skip)]
        MultipartRejection,
    ),
    #[error("multipart error: {0}")]
    MultipartError(
        #[from]
        #[serde(skip)]
        MultipartError,
    ),
}

impl IntoResponse for MultipartFormError {
    fn into_response(self) -> Response {
        (
            self.error_code(),
            axum::Json(serde_json::json!({ "error": format!("{}", self), "error_type": self })),
        )
            .into_response()
    }
}

#[async_trait]
impl<S, B, F, const CLL: u64> FromRequest<S, B> for MultipartForm<F, CLL>
where
    S: Send + Sync,
    B: Send + 'static,
    F: DeserializeOwned + Send,
    Multipart: FromRequest<S, B, Rejection = MultipartRejection>,
{
    type Rejection = MultipartFormError;

    async fn from_request(req: Request<B>, state: &S) -> Result<Self, MultipartFormError> {
        let mut multipart = Multipart::from_request(req, state).await?;
        let mut form = HashMap::new();
        let mut file = None;

        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(|_| MultipartFormError::InvalidField)?
        {
            let name = if let Some(name) = field.name() {
                name
            } else {
                continue;
            };
            if name == "file" {
                if field.file_name().is_none() {
                    continue;
                }
                let name = field.file_name().unwrap_or("").to_string();
                let bytes = field.bytes().await?.clone();
                file = Some(File { name, bytes });
            } else {
                form.insert(name.to_string(), field.text().await?);
            }
        }

        // Yes, this is silly, but it's convenient!
        let form: F = serde_json::from_value(serde_json::to_value(form)?)?;

        Ok(Self { form, file })
    }
}

use std::{fmt, str::FromStr};

use serde::{de, Deserialize, Deserializer};

fn empty_string_as_none<'de, D, T>(de: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
    T::Err: fmt::Display,
{
    let opt = Option::<String>::deserialize(de)?;
    match opt.as_deref() {
        None | Some("") => Ok(None),
        Some(s) => FromStr::from_str(s).map_err(de::Error::custom).map(Some),
    }
}
