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


const DB_URL_RW: &str = "postgres://postgres:a2hYF7kMexX2AMa9Rp2dMcFaP@marche-instance-1.cpdsugir6lq3.us-east-2.rds.amazonaws.com";
// TODO: Move to something that won't be committed. It's not a huge deal, the server shouldn't be public.
 const DB_URL_RO: &str = "postgres://postgres:a2hYF7kMexX2AMa9Rp2dMcFaP@marche.cluster-ro-cpdsugir6lq3.us-east-2.rds.amazonaws.com:5432/marche";
// const DB_URL_RW: &str = "postgres://postgres:a2hYF7kMexX2AMa9Rp2dMcFaP@marche.cluster-cpdsugir6lq3.us-east-2.rds.amazonaws.com:5432/marche";


pub fn establish_db_connection() -> PgConnection {
    // let database_url = env::var("DATABASE_URL").unwrap_or(DB_URL.to_string());
    PgConnection::establish(DB_URL_RW).unwrap()
}

pub fn establish_db_connection_ro() -> PgConnection {
    // let database_url = env::var("DATABASE_URL").unwrap_or(DB_URL.to_string());
    PgConnection::establish(DB_URL_RO).unwrap()
}
