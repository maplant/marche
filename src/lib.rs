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

const DB_URL: &str = "postgres://localhost:5432/forum";

pub fn establish_db_connection() -> Option<PgConnection> {
    let database_url = env::var("DATABASE_URL").unwrap_or(DB_URL.to_string());
    PgConnection::establish(&database_url).ok()
}
