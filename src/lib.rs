#[macro_use]
extern crate diesel;
#[macro_use]
extern crate maplit;

pub mod error;
pub mod items;
pub mod schema;
pub mod threads;
pub mod users;

use diesel::pg::PgConnection;
use diesel::Connection;
use std::env;

const DB_URL: &str = "postgres://postgres:test@localhost:5432/marche";

// TODO: No real reason to handle this gracefully.
pub fn establish_db_connection() -> Option<PgConnection> {
    let database_url = env::var("DATABASE_URL").unwrap_or(DB_URL.to_string());
    Some(PgConnection::establish(&database_url).unwrap())
    // PgConnection::establish(&database_url).ok()
}
