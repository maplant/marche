#[macro_use]
extern crate diesel;

pub mod items;
pub mod threads;
pub mod users;

use diesel::pg::PgConnection;
use diesel::Connection;
use std::env;
use askama::Template;

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
