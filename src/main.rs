use marche_server::{error, items, threads, users};
use rocket::fs::FileServer;
use rocket::response::Redirect;
use rocket::{catch, catchers, uri};
use rocket_dyn_templates::*;

#[catch(401)]
fn unauthorized() -> Redirect {
    Redirect::to(uri!(users::login_form()))
}

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
                users::leaderboard,
                users::login_action,
                users::login_form,
                error::error,
            ],
        )
        .mount("/static", FileServer::from("static/"))
        .register("/", catchers![unauthorized])
}
