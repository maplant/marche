//! Display threads
use crate::items::{Item, ItemDrop, ItemThumbnail};
use crate::schema::{replies, threads};
use crate::users::{User, UserCache, UserProfile};
use chrono::{prelude::*, NaiveDateTime};
use diesel::prelude::*;
use pulldown_cmark::{html, Options, Parser};
use rocket::form::Form;
use rocket::http::{Cookie, CookieJar};
use rocket::response::Redirect;
use rocket::{uri, FromForm};
use rocket_dyn_templates::Template;
use serde::Serialize;

#[derive(Queryable)]
pub struct Thread {
    /// Id of the thread
    pub id: i32,
    /// Date of the latest post
    pub last_post: NaiveDateTime,
    /// Title of the thread
    pub title: String,
}

#[derive(Insertable)]
#[table_name = "threads"]
pub struct NewThread<'t> {
    title: &'t str,
    last_post: NaiveDateTime,
}

const THREADS_PER_PAGE: i64 = 10;
const DATE_FMT: &str = "%m/%d %I:%M %P";

#[rocket::get("/")]
pub fn index(_user: User, cookies: &CookieJar<'_>) -> Template {
    use crate::schema::threads::dsl::*;

    #[derive(Serialize)]
    struct ThreadLink {
        num: usize,
        id: i32,
        title: String,
        date: String,
        new_posts: bool,
    }

    let conn = crate::establish_db_connection();

    let posts: Vec<_> = threads
        .order(last_post.desc())
        .limit(THREADS_PER_PAGE)
        .load::<Thread>(&conn)
        .ok()
        .unwrap_or_else(Vec::new)
        .into_iter()
        .enumerate()
        .map(|(i, thread)| {
            let date = thread.last_post.format(DATE_FMT).to_string();
            ThreadLink {
                num: i + 1,
                id: thread.id,
                title: thread.title,
                // Check if there are any new posts, by checking if the last_seen_{thread_id}
                // cookie is the same date as the last post.
                new_posts: cookies
                    .get(&format!("last_seen_{}", thread.id))
                    .map(|d| d.value() != date)
                    .unwrap_or(true),
                date,
            }
        })
        .collect();

    Template::render(
        "index",
        hashmap! {
            "posts" => posts,
        },
    )
}

#[rocket::get("/thread/<thread_id>?<error>")]
pub fn thread(
    user: User,
    cookies: &CookieJar<'_>,
    thread_id: i32,
    error: Option<&str>,
) -> Template {
    use crate::schema::replies;
    use crate::schema::threads::dsl::*;

    #[derive(Serialize)]
    struct Reward {
        name: String,
        rarity: String,
    }

    #[derive(Serialize)]
    struct Post {
        id: i32,
        author: UserProfile,
        body: String,
        date: String,
        reactions: Vec<ItemThumbnail>,
        reward: Option<Reward>,
        can_react: bool,
    }

    #[derive(Serialize)]
    struct Context<'t, 'e> {
        id: i32,
        title: &'t str,
        posts: Vec<Post>,
        error: Option<&'e str>,
    }

    let conn = crate::establish_db_connection();
    let mut user_cache = UserCache::new(&conn);
    let post_title = &threads
        .filter(id.eq(thread_id))
        .first::<Thread>(&conn)
        .unwrap()
        .title;

    let posts = replies::dsl::replies
        .filter(replies::dsl::thread_id.eq(thread_id))
        .order(replies::dsl::post_date.asc())
        .load::<Reply>(&conn)
        .unwrap()
        .into_iter()
        .map(|t| Post {
            id: t.id,
            author: user_cache.get(t.author_id).clone(),
            body: t.body,
            date: t.post_date.format(DATE_FMT).to_string(),
            reactions: t
                .reactions
                .into_iter()
                .map(|d| ItemDrop::fetch(&conn, d).thumbnail(&conn))
                .collect(),
            reward: t.reward.map(|r| {
                let item = Item::fetch(&conn, r);
                Reward {
                    name: item.name,
                    rarity: item.rarity.to_string(),
                }
            }),
            can_react: t.author_id == user.id,
        })
        .collect::<Vec<_>>();

    // Update the last_seen_{thread_id} cookie to the latest date.
    posts.last().map(|last| {
        cookies.add(Cookie::new(
            format!("last_seen_{}", thread_id),
            last.date.clone(),
        ))
    });

    Template::render(
        "thread",
        Context {
            id: thread_id,
            title: post_title,
            posts,
            error,
        },
    )
}

// TODO: Make error a vec of strs.
#[rocket::get("/author?<error>")]
pub fn author_form(_user: User, error: Option<&str>) -> Template {
    #[derive(Serialize)]
    struct Context<'e> {
        error: Option<&'e str>,
    }

    Template::render("author", Context { error })
}

#[derive(FromForm)]
pub struct NewThreadReq {
    title: String,
    body: String,
}

#[rocket::post("/author", data = "<thread>")]
pub fn author_action(user: User, thread: Form<NewThreadReq>) -> Redirect {
    use crate::schema::{replies, threads};

    if thread.title.is_empty() || thread.body.is_empty() {
        return Redirect::to(uri!(author_form(Some(
            "Thread title or body cannot be empty"
        ))));
    }

    let conn = crate::establish_db_connection();
    let post_date = Utc::now().naive_utc();

    // Parse the body as markdown
    let mut html_output = String::with_capacity(thread.body.len() * 3 / 2);
    let parser = Parser::new_ext(&thread.body, Options::empty());
    html::push_html(&mut html_output, parser);

    let thread: Thread = diesel::insert_into(threads::table)
        .values(&NewThread {
            last_post: post_date,
            title: &thread.title,
        })
        .get_result(&conn)
        .unwrap();

    // Make first reply
    // TODO: Move into transaction so that you can't post a thread
    // without the first reply.
    let _: Reply = diesel::insert_into(replies::table)
        .values(&NewReply {
            author_id: user.id,
            thread_id: thread.id,
            post_date,
            body: &html_output,
            reward: ItemDrop::drop(&conn, &user).map(ItemDrop::item_id),
            reactions: Vec::new(),
        })
        .get_result(&conn)
        .unwrap();

    Redirect::to(uri!(thread(thread.id, Option::<&str>::None)))
}

#[derive(Queryable)]
pub struct Reply {
    /// Id of the reply
    pub id: i32,
    /// Id of the author
    pub author_id: i32,
    /// Id of the thread
    pub thread_id: i32,
    /// Date of posting
    pub post_date: NaiveDateTime,
    /// Body of the reply
    pub body: String,
    /// Any item that was rewarded for this post
    pub reward: Option<i32>,
    /// Reactions attached to this post
    pub reactions: Vec<i32>,
}

impl Reply {
    pub fn fetch(conn: &PgConnection, reply_id: i32) -> Self {
        use crate::schema::replies::dsl::*;
        replies
            .filter(id.eq(reply_id))
            .first::<Reply>(conn)
            .unwrap()
    }
}

#[derive(Insertable)]
#[table_name = "replies"]
pub struct NewReply<'b> {
    author_id: i32,
    thread_id: i32,
    post_date: NaiveDateTime,
    body: &'b str,
    reward: Option<i32>,
    reactions: Vec<i32>,
}

#[derive(FromForm)]
pub struct ReplyReq {
    reply: String,
}

#[rocket::post("/reply/<thread_id>", data = "<reply>")]
pub fn reply_action(user: User, reply: Form<ReplyReq>, thread_id: i32) -> Redirect {
    use crate::schema::{replies, threads};

    let conn = crate::establish_db_connection();
    let post_date = Utc::now().naive_utc();

    if reply.reply.trim().is_empty() {
        return Redirect::to(format!(
            "{}#reply",
            uri!(thread(thread_id, Some("Cannot post an empty reply")))
        ));
    }

    // Parse the body as markdown
    let mut html_output = String::with_capacity(reply.reply.len() * 3 / 2);
    let parser = Parser::new_ext(&reply.reply, Options::empty());
    html::push_html(&mut html_output, parser);

    let reply: Reply = diesel::insert_into(replies::table)
        .values(&NewReply {
            author_id: user.id,
            thread_id,
            post_date,
            body: &html_output,
            reward: ItemDrop::drop(&conn, &user).map(ItemDrop::item_id),
            reactions: Vec::new(),
        })
        .get_result(&conn)
        .unwrap();

    let _: Result<Thread, _> = diesel::update(threads::table)
        .filter(threads::dsl::id.eq(thread_id))
        .set(threads::dsl::last_post.eq(post_date))
        .get_result(&conn);

    Redirect::to(format!(
        "{}#{}",
        uri!(thread(thread_id, Option::<&str>::None)),
        reply.id
    ))
}
