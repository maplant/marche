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

pub fn establish_db_connection() -> PgConnection {
    let database_url = env::var("DATABASE_URL").unwrap();
    PgConnection::establish(&database_url).unwrap()
}
