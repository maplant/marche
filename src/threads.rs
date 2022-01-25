//! Display threads
use crate::items::{IncomingOffer, ItemDrop, ItemThumbnail};
use crate::users::{User, UserCache, UserProfile};
use chrono::{prelude::*, NaiveDateTime};
use diesel::prelude::*;
use pulldown_cmark::{html, Options, Parser};
use rocket::form::Form;
use rocket::http::uri::fmt::Path;
use rocket::http::uri::Segments;
use rocket::http::{Cookie, CookieJar};
use rocket::request::FromSegments;
use rocket::response::Redirect;
use rocket::{uri, FromForm};
use rocket_dyn_templates::Template;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

table! {
    threads(id) {
        id -> Integer,
        last_post -> Timestamp,
        title -> Text,
        tags -> Array<Integer>,
    }
}

#[derive(Queryable)]
pub struct Thread {
    /// Id of the thread
    pub id: i32,
    /// Date of the latest post
    pub last_post: NaiveDateTime,
    /// Title of the thread
    pub title: String,
    /// Tags given to this thread
    pub tags: Vec<i32>,
}

#[derive(Insertable)]
#[table_name = "threads"]
pub struct NewThread<'t> {
    title: &'t str,
    tags: Vec<i32>,
    last_post: NaiveDateTime,
}

const THREADS_PER_PAGE: i64 = 10;
const DATE_FMT: &str = "%m/%d %I:%M %P";
const MINUTES_TIMESTAMP_IS_EMPHASIZED: i64 = 60 * 24;

#[rocket::post("/remove-tag/<name>", data = "<tags>")]
pub fn remove_tag(_user: User, mut tags: Form<HashMap<String, String>>, name: &str) -> Redirect {
    let _ = tags.remove("add-tag");
    let tags = tags
        .iter()
        .filter_map(|(tname, _)| (tname != name).then(|| tname))
        .fold(String::new(), |prefix, suffix| prefix + suffix.trim() + "/");
    Redirect::to(format!("/t/{}", tags))
}

#[rocket::post("/add-tag", data = "<tags>")]
pub fn add_tag(_user: User, mut tags: Form<HashMap<String, String>>) -> Redirect {
    let add_tag = clean_tag_name(&tags.remove("add-tag").unwrap_or_else(String::new));
    let tags = tags.iter().fold(String::new(), |prefix, suffix| {
        prefix + suffix.0.trim() + "/"
    });
    Redirect::to(format!("/t/{}/{}", add_tag, tags))
}

#[rocket::get("/t/<viewed_tags..>")]
pub fn view_tags(user: User, mut viewed_tags: Tags, cookies: &CookieJar<'_>) -> Template {
    use self::threads::dsl::*;

    #[derive(Serialize)]
    struct Context {
        tags: Vec<Tag>,
        posts: Vec<ThreadLink>,
        curr_path: String,
        offer_count: i64,
    }

    #[derive(Serialize)]
    struct ThreadLink {
        num: usize,
        id: i32,
        title: String,
        date: String,
        emphasize_date: bool,
        new_posts: bool,
        tags: Vec<String>,
    }

    let conn = crate::establish_db_connection();

    if viewed_tags.is_empty() {
        viewed_tags = Tags {
            tags: vec![Tag::fetch_if_exists(&conn, "en").unwrap()],
        };
    }

    let posts: Vec<_> = threads
        .filter(tags.contains(viewed_tags.clone().into_id_vec()))
        .order(last_post.desc())
        .limit(THREADS_PER_PAGE)
        .load::<Thread>(&conn)
        .ok()
        .unwrap_or_else(Vec::new)
        .into_iter()
        .enumerate()
        .map(|(i, thread)| {
            let date = thread.last_post.format(DATE_FMT).to_string();

            // TODO: Consider moving duration->plaintext into common utility
            let duration_since_last_post = Utc::now().naive_utc() - thread.last_post;
            let duration_min = duration_since_last_post.num_minutes();
            let duration_hours = duration_since_last_post.num_hours();
            let duration_days = duration_since_last_post.num_days();
            let duration_weeks = duration_since_last_post.num_weeks();
            let duration_string: String = if duration_weeks > 0 {
                format!(
                    "{} week{} ago",
                    duration_weeks,
                    if duration_weeks > 1 { "s" } else { "" }
                )
            } else if duration_days > 0 {
                format!(
                    "{} day{} ago",
                    duration_days,
                    if duration_days > 1 { "s" } else { "" }
                )
            } else if duration_hours > 0 {
                format!(
                    "{} hour{} ago",
                    duration_hours,
                    if duration_hours > 1 { "s" } else { "" }
                )
            } else if duration_min >= 5 {
                format!(
                    "{} minute{} ago",
                    duration_min,
                    if duration_min > 1 { "s" } else { "" }
                )
            } else {
                String::from("just now!")
            };

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
                date: duration_string,
                emphasize_date: duration_min < MINUTES_TIMESTAMP_IS_EMPHASIZED,
                tags: thread
                    .tags
                    .into_iter()
                    .filter_map(|tid| Tag::fetch_from_id(&conn, tid))
                    .map(|t| t.name)
                    .collect(),
            }
        })
        .collect();

    Template::render(
        "index",
        Context {
            curr_path: viewed_tags.fmt(),
            tags: viewed_tags.tags,
            posts: posts,
            offer_count: IncomingOffer::count(&conn, &user),
        },
    )
}

#[rocket::get("/")]
pub fn index(_user: User) -> Redirect {
    // TODO: change this to the user's preferred default language.
    Redirect::to(format!("/t/en"))
}

#[rocket::get("/thread/<thread_id>?<error>")]
pub fn thread(
    user: User,
    cookies: &CookieJar<'_>,
    thread_id: i32,
    error: Option<&str>,
) -> Template {
    use self::replies;
    use self::threads::dsl::*;

    // Consider removing this in favor of just using ItemThumbnail instead
    #[derive(Serialize)]
    struct Reward {
        thumbnail: String,
        name: String,
        description: String,
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
        offer_count: i64,
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
                let drop = ItemDrop::fetch(&conn, r);
                let item = drop.fetch_item(&conn);
                Reward {
                    name: item.name,
                    description: item.description,
                    thumbnail: drop.thumbnail_html(&conn),
                    rarity: item.rarity.to_string(),
                }
            }),
            can_react: t.author_id != user.id,
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
            offer_count: IncomingOffer::count(&conn, &user),
            error,
        },
    )
}

// TODO: Make error a vec of strs.
#[rocket::get("/author?<error>")]
pub fn author_form(user: User, error: Option<&str>) -> Template {
    #[derive(Serialize)]
    struct Context<'e> {
        error: Option<&'e str>,
        offer_count: i64,
    }

    let conn = crate::establish_db_connection();
    Template::render(
        "author",
        Context {
            error,
            offer_count: IncomingOffer::count(&conn, &user),
        },
    )
}

#[derive(FromForm)]
pub struct NewThreadReq {
    title: String,
    tags: String,
    body: String,
}

#[rocket::post("/author", data = "<thread>")]
pub fn author_action(user: User, thread: Form<NewThreadReq>) -> Redirect {
    use self::{replies, threads};

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

    let tags = parse_tag_list(&thread.tags)
        .filter_map(|t| Tag::fetch_and_inc(&conn, t))
        .map(|t| t.id())
        .collect();

    let thread: Thread = diesel::insert_into(threads::table)
        .values(&NewThread {
            last_post: post_date,
            title: &thread.title,
            tags,
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
            reward: ItemDrop::drop(&conn, &user).map(|d| d.id),
            reactions: Vec::new(),
        })
        .get_result(&conn)
        .unwrap();

    Redirect::to(uri!(thread(thread.id, Option::<&str>::None)))
}

table! {
    tags(id) {
        id -> Integer,
        name -> Text,
        num_tagged -> Integer,
    }
}

#[derive(Debug, Queryable, Serialize, Clone)]
pub struct Tag {
    pub id: i32,
    pub name: String,
    /// Number of posts that have been tagged with this tag.
    pub num_tagged: i32,
}

#[derive(Insertable)]
#[table_name = "tags"]
pub struct NewTag<'n> {
    name: &'n str,
}

impl Tag {
    pub fn id(&self) -> i32 {
        self.id
    }

    /// Returns the most popular tags.
    pub fn popular(conn: &PgConnection) -> Vec<Self> {
        use self::tags::dsl::*;

        tags.order(num_tagged.desc())
            .limit(10)
            .load::<Self>(conn)
            .unwrap_or_default()
    }

    /// Fetches a tag, creating it if it doesn't already exist. num_tagged is incremented
    /// or set to one.
    pub fn fetch_and_inc(conn: &PgConnection, tag: &str) -> Option<Self> {
        use self::tags::dsl::*;

        let tag = clean_tag_name(tag);
        if tag.is_empty() {
            return None;
        }

        // TODO: make this an insert with an ON CONFLICT
        diesel::insert_into(tags)
            .values(&NewTag { name: &tag })
            .on_conflict(name)
            .do_update()
            .set(num_tagged.eq(num_tagged + 1))
            .get_result(conn)
            .ok()
    }

    pub fn fetch_from_id(conn: &PgConnection, tag_id: i32) -> Option<Self> {
        use self::tags::dsl::*;

        tags.filter(id.eq(tag_id)).first::<Self>(conn).ok()
    }

    /// Fetches a tag only if that tag already exists.
    pub fn fetch_if_exists(conn: &PgConnection, tag: &str) -> Option<Self> {
        use self::tags::dsl::*;

        tags.filter(name.eq(clean_tag_name(tag)))
            .first::<Self>(conn)
            .ok()
    }
}

fn clean_tag_name(name: &str) -> String {
    name.trim().to_lowercase()
}

#[derive(Debug, Clone)]
pub struct Tags {
    tags: Vec<Tag>,
}

impl Tags {
    /// Returns the most popular tags.
    pub fn popular(conn: &PgConnection) -> Self {
        use self::tags::dsl::*;

        Self {
            tags: tags
                .order(num_tagged.desc())
                .limit(10)
                .load::<Tag>(conn)
                .unwrap_or_default(),
        }
    }

    pub fn fmt(&self) -> String {
        self.tags
            .iter()
            .fold(String::new(), |prefix, suffix| prefix + &suffix.name + "/")
    }

    pub fn push(&mut self, tag: Tag) {
        self.tags.push(tag);
    }

    pub fn into_id_vec(self) -> Vec<i32> {
        self.into()
    }

    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }
}

impl Into<Vec<i32>> for Tags {
    fn into(self) -> Vec<i32> {
        self.tags.into_iter().map(|x| x.id).collect()
    }
}

impl<'r> FromSegments<'r> for Tags {
    type Error = crate::DbConnectionFailure;

    fn from_segments(segments: Segments<'r, Path>) -> Result<Self, Self::Error> {
        let conn = crate::establish_db_connection();
        let mut seen = HashSet::new();
        let tags = segments
            .filter_map(|s| {
                Tag::fetch_if_exists(&conn, s)
                    .map(|t| seen.insert(t.id).then(|| t))
                    .flatten()
            })
            .collect::<Vec<_>>();
        Ok(Tags { tags })
    }
}

fn parse_tag_list(list: &str) -> impl Iterator<Item = &str> {
    // TODO: More stuff!
    list.split(",").map(|i| i.trim())
}

table! {
    replies(id) {
        id -> Integer,
        author_id -> Integer,
        thread_id -> Integer,
        post_date -> Timestamp,
        body -> Text,
        reward -> Nullable<Integer>,
        reactions -> Array<Integer>,
    }
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
    pub fn fetch(conn: &PgConnection, reply_id: i32) -> Result<Self, diesel::result::Error> {
        use self::replies::dsl::*;

        replies.filter(id.eq(reply_id)).first::<Reply>(conn)
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
    use self::{replies, threads};

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
            reward: ItemDrop::drop(&conn, &user).map(|d| d.id),
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
