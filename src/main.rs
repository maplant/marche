use rocket::fs::FileServer;
use rocket_dyn_templates::*;
use server::{error, items, threads, users};

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
                items::item,
                items::offer,
                items::offer_action,
                items::offers,
                items::decline,
                items::accept,
                items::react,
                items::react_action,
                users::equip,
                users::curr_profile,
                users::profile,
                users::login_action,
                users::login_form,
                error::error,
                threads::unauthorized,
            ],
        )
        .mount("/static", FileServer::from("static/"))
}
