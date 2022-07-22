#[macro_use]
extern crate diesel;

pub mod items;
pub mod threads;
pub mod users;

use std::{collections::HashMap, env, error::Error as StdError};

use askama::Template;
use axum::{
    async_trait,
    body::Bytes,
    extract::{
        multipart::MultipartError, ContentLengthLimit, FromRequest, Multipart, RequestParts,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use derive_more::From;
use diesel::{pg::PgConnection, Connection};
use serde::{de::DeserializeOwned, Serialize};

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
