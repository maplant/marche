use rocket::fs::FileServer;
use rocket_dyn_templates::*;
use serde::{Deserialize, Serialize};
use server::items::Rarity;
use server::{error, threads, users};
use std::path::{Path, PathBuf};

#[rocket::launch]
fn launch_server() -> _ {
    rocket::build()
        .attach(Template::fairing())
        .mount(
            "/",
            rocket::routes![
                threads::index,
                threads::thread,
                threads::author_form,
                threads::author_action,
                threads::author_unauthorized,
                threads::reply_action,
                users::login_action,
                users::login_form,
                error::error,
                threads::unauthorized, 
           ],
        )
        .mount("/static", FileServer::from("static/"))
}
