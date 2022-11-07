pub mod items;
pub mod pages;
pub mod threads;
pub mod users;

use std::{any::Any, collections::HashMap, error::Error as StdError};

use askama::Template;
use axum::{
    async_trait,
    body::{Body, Bytes},
    extract::{
        multipart::MultipartError, ContentLengthLimit, Extension, FromRequest, Multipart,
        RequestParts,
    },
    handler::Handler,
    http::StatusCode,
    response::{IntoResponse, Response},
    Router,
};
use derive_more::{Display, From};
use serde::{de::DeserializeOwned, Serialize};
use sqlx::PgPool;

pub const DATE_FMT: &str = "%B %-d, %Y at %I:%M %P";

pub struct Endpoint {
    route_type: RouteType,
    path:       &'static str,
    handler:    &'static (dyn Any + Send + Sync + 'static),
    installer: fn(
        RouteType,
        &'static str,
        &'static (dyn Any + Send + Sync + 'static),
        Router<Body>,
    ) -> Router<Body>,
}

impl Endpoint {
    pub const fn new<I, A>(route_type: RouteType, path: &'static str, handler: &'static I) -> Self
    where
        I: Handler<A, Body> + Copy + Any + Send + Sync + 'static,
        A: 'static,
    {
        Self {
            path,
            route_type,
            handler: handler as &(dyn Any + Send + Sync + 'static),
            installer: install::<I, A>,
        }
    }

    pub fn install(&self, router: Router<Body>) -> Router<Body> {
        (self.installer)(self.route_type, self.path, self.handler, router)
    }
}

#[derive(Copy, Clone, Display)]
pub enum RouteType {
    #[display(fmt = "GET")]
    Get,
    #[display(fmt = "POST")]
    Post,
}

inventory::collect!(Endpoint);

pub fn install<I, A>(
    route_type: RouteType,
    path: &'static str,
    handler: &'static (dyn Any + Send + Sync + 'static),
    router: Router<Body>,
) -> Router<Body>
where
    I: Handler<A, Body> + Copy,
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

#[derive(Serialize, From)]
pub enum MultipartFormError {
    InvalidContentLength,
    InvalidField,
    MultipartError(#[serde(skip)] MultipartError),
}

impl IntoResponse for MultipartFormError {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, "internal service error").into_response()
    }
}

#[async_trait]
impl<F, B, const CLL: u64> FromRequest<B> for MultipartForm<F, CLL>
where
    B: Send,
    F: DeserializeOwned + Send,
    Multipart: FromRequest<B>,
    ContentLengthLimit<Multipart, CLL>: FromRequest<B>,
    <ContentLengthLimit<Multipart, CLL> as FromRequest<B>>::Rejection:
        StdError + Send + Sync + 'static,
{
    type Rejection = MultipartFormError;

    async fn from_request(req: &mut RequestParts<B>) -> Result<Self, MultipartFormError> {
        let ContentLengthLimit(mut multipart) =
            ContentLengthLimit::<Multipart, CLL>::from_request(req)
                .await
                .map_err(|_| MultipartFormError::InvalidContentLength)?;
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
        let form: F = serde_json::from_value(serde_json::to_value(form).unwrap()).unwrap();

        Ok(Self { form, file })
    }
}

#[derive(Template)]
#[template(path = "404.html")]
pub struct NotFound {
    offers: i64,
}

impl NotFound {
    pub async fn show(pool: Extension<PgPool>, user: users::User) -> Self {
        /*
        let conn = pool.get().expect("Could not connect to db");
        Self {
            offers: user.incoming_offers(&conn),
    }
         */
        Self {
            offers: 0
        }
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
