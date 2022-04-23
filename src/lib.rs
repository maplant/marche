#[macro_use]
extern crate diesel;

pub mod items;
pub mod threads;
pub mod users;

use anyhow::Error;
use askama::Template;
use axum::body::Bytes;
use axum::extract::{ContentLengthLimit, FromRequest, Multipart, RequestParts};
use axum::response::{IntoResponse, Response};
use axum::{async_trait, Json};
use diesel::pg::PgConnection;
use diesel::Connection;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashMap;
use std::env;
use std::error::Error as StdError;

/// A multipart form that includes a file (which must be named "file").
/// Ideally we'd like this to be
#[derive(Debug)]
pub struct MultipartForm<Form, const N: u64> {
    pub form: Form,
    pub file: Option<File>,
}

#[derive(Debug)]
pub struct File {
    pub name: String,
    pub bytes: Bytes,
}

#[derive(Serialize, Debug)]
pub struct JsonError {
    error: String,
}

impl JsonError {
    pub fn new(error: String) -> Self {
        Self { error }
    }
}

#[macro_export]
macro_rules! bail {
    ($msg:literal $(,)?) => {
        return Err(JsonError::new(format!($msg)))
    };
    ($err:expr $(,)?) => {
        return Err(JsonError::new(format!($err)))
    };
    ($fmt:expr, $($arg:tt)*) => {
        return Err(JsonError::new(format!($fmt, $($arg)*)))
    };
}

#[macro_export]
macro_rules! error {
    ($msg:literal $(,)?) => {
        JsonError::new(format!($msg))
    };
    ($err:expr $(,)?) => {
        JsonError::new(format!($err))
    };
    ($fmt:expr, $($arg:tt)*) => {
        JsonError::new(format!($fmt, $($arg)*))
    };
}

impl<E> From<E> for JsonError
where
    E: StdError + Send + Sync + 'static,
{
    fn from(e: E) -> Self {
        JsonError {
            error: format!("{}", Error::from(e)),
        }
    }
}

impl IntoResponse for JsonError {
    fn into_response(self) -> Response {
        Json(self).into_response()
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
    type Rejection = JsonError;

    async fn from_request(req: &mut RequestParts<B>) -> Result<Self, Self::Rejection> {
        let ContentLengthLimit(mut multipart) =
            ContentLengthLimit::<Multipart, CLL>::from_request(req).await?;
        let mut form = HashMap::new();
        let mut file = None;

        while let Some(field) = multipart.next_field().await? {
            let name = if let Some(name) = field.name() {
                name
            } else {
                continue;
            };
            if name == "file" {
                let name = field.file_name().unwrap_or("").to_string();
                let bytes = field.bytes().await?.clone();
                file = Some(File { name, bytes });
            } else {
                form.insert(name.to_string(), field.text().await?);
            }
        }

        // Yes, this is silly, but it's convenient!
        let form: F = serde_json::from_value(serde_json::to_value(form).unwrap())?;

        Ok(Self { form, file })
    }
}

#[derive(serde::Deserialize)]
pub struct ErrorMessage {
    error: Option<String>,
}

pub fn establish_db_connection() -> PgConnection {
    let database_url = env::var("DATABASE_URL").unwrap();
    PgConnection::establish(&database_url).unwrap()
}

#[derive(Debug)]
pub struct DbConnectionFailure;

#[derive(Template)]
#[template(path = "404.html")]
pub struct NotFound {
    offers: i64,
}

impl NotFound {
    pub fn new(offers: i64) -> Self {
        Self { offers }
    }

    pub async fn show(user: users::User) -> Self {
        let conn = establish_db_connection();
        Self {
            offers: user.incoming_offers(&conn),
        }
    }
}
