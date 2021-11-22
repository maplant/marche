//! Display threads
use crate::items::{Item, ItemDrop};
use crate::schema::{replies, threads};
use crate::users::{login_form, User, UserCache};
use chrono::{prelude::*, NaiveDateTime};
use diesel::prelude::*;
use rocket::form::Form;
use rocket::response::Redirect;
use rocket::{uri, FromForm};
use rocket_dyn_templates::Template;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Queryable)]
pub struct Thread {
    /// Id of the thread
    id: i32,
    /// Id of the author
    author_id: i32,
    /// Date of posting
    post_date: NaiveDateTime,
    /// Date of the latest post
    last_post: NaiveDateTime,
    /// Title of the thread
    title: String,
    /// Body of the thread, rendered from markup
    body: String,
    /// Any item that was rewarded for this post
    reward: Option<i32>,
}

#[derive(Insertable)]
#[table_name = "threads"]
pub struct NewThread<'t, 'b> {
    author_id: i32,
    post_date: NaiveDateTime,
    last_post: NaiveDateTime,
    title: &'t str,
    body: &'b str,
    reward: Option<i32>,
}

const THREADS_PER_PAGE: i64 = 10;
const DATE_FMT: &str = "%m/%d %I:%M %P";

#[rocket::get("/")]
pub fn index(_user: User) -> Template {
    use crate::schema::threads::dsl::*;

    #[derive(Serialize)]
    struct ThreadLink {
        num: usize,
        id: i32,
        title: String,
        date: String,
    }

    let conn = super::establish_db_connection().unwrap();

    let posts: Vec<_> = threads
        .order(last_post.desc())
        .limit(THREADS_PER_PAGE)
        .load::<Thread>(&conn)
        .ok()
        .unwrap_or_else(Vec::new)
        .into_iter()
        .enumerate()
        .map(|(i, thread)| ThreadLink {
            num: i + 1,
            id: thread.id,
            title: thread.title,
            date: thread.last_post.format(DATE_FMT).to_string(),
        })
        .collect();

    Template::render(
        "index",
        hashmap! {
            "posts" => posts,
        },
    )
}

#[rocket::get("/", rank = 2)]
pub fn unauthorized() -> Redirect {
    Redirect::to(rocket::uri!(crate::users::login_form()))
}

#[rocket::get("/thread/<thread_id>")]
pub fn thread(user: User, thread_id: i32) -> Template {
    use crate::schema::replies;
    use crate::schema::threads::dsl::*;

    #[derive(Serialize)]
    struct Reward {
        name: String,
    }

    #[derive(Serialize)]
    struct Post {
        author: String,
        body: String,
        date: String,
        reward: Option<Reward>,
    }
    #[derive(Serialize)]
    struct Context {
        id: i32,
        title: String,
        posts: Vec<Post>,
    }

    let conn = crate::establish_db_connection().unwrap();
    let mut user_cache = UserCache::new(&conn);
    let mut post_title = String::new();
    let mut posts: Vec<_> = threads
        .filter(id.eq(thread_id))
        .load::<Thread>(&conn)
        .unwrap()
        .into_iter()
        .map(|t| {
            post_title = t.title;
            Post {
                author: user_cache.get(t.author_id).name.clone(),
                body: t.body,
                date: t.post_date.format(DATE_FMT).to_string(),
                reward: t.reward.map(|r| Reward {
                    name: Item::fetch(&conn, r).name,
                }),
            }
        })
        .collect();

    posts.extend(
        replies::dsl::replies
            .filter(replies::dsl::thread_id.eq(thread_id))
            .load::<Reply>(&conn)
            .unwrap()
            .into_iter()
            .map(|t| Post {
                author: user_cache.get(t.author_id).name.clone(),
                body: t.body,
                date: t.post_date.format(DATE_FMT).to_string(),
                reward: t.reward.map(|r| Reward {
                    name: Item::fetch(&conn, r).name,
                }),
            })
            .collect::<Vec<_>>(),
    );

    Template::render(
        "thread",
        Context {
            id: thread_id,
            title: post_title,
            posts,
        },
    )
}

#[rocket::get("/author")]
pub fn author_form(user: User) -> Template {
    Template::render("author/thread", HashMap::<String, String>::new())
}

#[derive(FromForm)]
pub struct NewThreadReq {
    title: String,
    body: String,
}

#[rocket::post("/author", data = "<thread>")]
pub fn author_action(user: User, thread: Form<NewThreadReq>) -> Redirect {
    use crate::schema::threads;

    let conn = match crate::establish_db_connection() {
        Some(conn) => conn,
        None => {
            return Redirect::to(uri!(crate::error::error(
                "Failed to establish database connection"
            )))
        }
    };
    let post_date = Utc::now().naive_utc();
    let new_thread = NewThread {
        author_id: user.id,
        post_date,
        last_post: post_date,
        title: &thread.title,
        body: &thread.body,
        reward: ItemDrop::drop(&conn, &user).map(ItemDrop::item_id),
    };
    let _: Result<Thread, _> = diesel::insert_into(threads::table)
        .values(&new_thread)
        .get_result(&conn);
    Redirect::to("/")
}

#[rocket::get("/author", rank = 2)]
pub fn author_unauthorized() -> Redirect {
    Redirect::to(rocket::uri!(crate::users::login_form()))
}

#[derive(Queryable)]
pub struct Reply {
    /// Id of the reply
    id: i32,
    /// Id of the author
    author_id: i32,
    /// Id of the thread
    thread_id: i32,
    /// Date of posting
    post_date: NaiveDateTime,
    /// Body of the reply
    body: String,
    /// Any item that was rewarded for this post
    reward: Option<i32>,
}

#[derive(Insertable)]
#[table_name = "replies"]
pub struct NewReply<'b> {
    author_id: i32,
    thread_id: i32,
    post_date: NaiveDateTime,
    body: &'b str,
    reward: Option<i32>,
}

#[derive(FromForm)]
pub struct ReplyReq {
    reply: String,
}

#[rocket::post("/reply/<thread_id>", data = "<reply>")]
pub fn reply_action(user: User, reply: Form<ReplyReq>, thread_id: i32) -> Redirect {
    use crate::schema::{replies, threads};

    let conn = match crate::establish_db_connection() {
        Some(conn) => conn,
        None => {
            return Redirect::to(uri!(crate::error::error(
                "Failed to establish database connection"
            )))
        }
    };

    let post_date = Utc::now().naive_utc();
    let new_reply = NewReply {
        author_id: user.id,
        thread_id,
        post_date,
        body: &reply.reply,
        reward: ItemDrop::drop(&conn, &user).map(ItemDrop::item_id),
    };

    let _: Result<Reply, _> = diesel::insert_into(replies::table)
        .values(&new_reply)
        .get_result(&conn);

    let _: Result<Thread, _> = diesel::update(threads::table)
        .set(threads::dsl::last_post.eq(post_date))
        .get_result(&conn);

    Redirect::to(uri!(thread(thread_id)))
}
