//! Display threads
use crate::schema::threads;
use crate::users::{login_form, User};
use chrono::NaiveDateTime;
use rocket::response::Redirect;
use rocket_dyn_templates::Template;

#[derive(Queryable)]
pub struct Thread {
    /// Id of the thread
    id: i32,
    /// Id of the author
    author_id: i32,
    /// Date of posting
    post_date: NaiveDateTime,
    /// Title of the thread
    title: String,
    /// Body of the thread, rendered from markup
    body: String,
}

#[derive(Insertable)]
#[table_name = "threads"]
pub struct NewThread<'t, 'b> {
    author_id: i32,
    post_date: NaiveDateTime,
    tile: &'t str,
    body: &'b str,
}

#[derive(Queryable)]
pub struct Reply {
    /// Id of the reply
    id: i32,
    /// Id of the thread
    thread_id: i32,
    /// Id of the author
    author_id: i32,
}

#[rocket::get("/")]
pub fn index(user: User) -> Template {
    Template::render(
        "index",
        hashmap! {
            "posts" => Vec::<String>::new(),
        },
    )
}

#[rocket::get("/", rank = 2)]
pub fn unauthorized() -> Redirect {
    Redirect::to(rocket::uri!(crate::users::login_form()))
}
