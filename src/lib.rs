#[macro_use]
extern crate diesel;

pub mod items;
pub mod threads;
pub mod users;

use diesel::pg::PgConnection;
use diesel::Connection;
use std::env;

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
