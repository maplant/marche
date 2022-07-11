//! Display threads
use std::collections::{HashMap, HashSet};

use askama::Template;
use axum::{
    body::Bytes,
    extract::{Form, Path},
    response::Redirect,
    Json,
};
use chrono::{prelude::*, NaiveDateTime};
use diesel::prelude::*;
use lazy_static::lazy_static;
use regex::{Captures, Regex};
use serde::{Deserialize, Serialize};

use crate::{
    bail, error,
    items::{IncomingOffer, ItemDrop, ItemThumbnail},
    users::{ProfileStub, User, UserCache},
    File, JsonError, MultipartForm, NotFound,
};

table! {
    threads(id) {
        id -> Integer,
        last_post -> Integer,
        title -> Text,
        tags -> Array<Integer>,
    }
}

use diesel::sql_types::{BigInt, Text};

sql_function!(fn nextval(x: Text) -> BigInt);
sql_function!(fn pg_get_serial_sequence(table: Text, column: Text) -> Text);

#[derive(Queryable, Debug, Serialize)]
pub struct Thread {
    /// Id of the thread
    pub id:        i32,
    /// Id of the last post
    pub last_post: i32,
    /// Title of the thread
    pub title:     String,
    /// Tags given to this thread
    pub tags:      Vec<i32>,
}

#[derive(Insertable)]
#[table_name = "threads"]
pub struct NewThread<'t> {
    id:        i32,
    title:     &'t str,
    tags:      Vec<i32>,
    last_post: i32,
}

const THREADS_PER_PAGE: i64 = 25;
const DATE_FMT: &str = "%B %-d at %I:%M %P";
const MINUTES_TIMESTAMP_IS_EMPHASIZED: i64 = 60 * 24;

// TODO: These next two functions absolutely should be done client side.
pub async fn remove_tag(
    _user: User,
    Path(name): Path<String>,
    Form(mut tags): Form<HashMap<String, String>>,
) -> Redirect {
    let _ = tags.remove("add-tag");
    let tags = tags
        .iter()
        .filter_map(|(tname, _)| (tname != &name).then(|| tname))
        .fold(String::new(), |prefix, suffix| {
            prefix + &*urlencoding::encode(suffix.trim()) + "/"
        });
    Redirect::to(format!("/t/{}", tags).parse().unwrap())
}

pub async fn add_tag(_user: User, Form(mut tags): Form<HashMap<String, String>>) -> Redirect {
    let add_tag = clean_tag_name(&tags.remove("add-tag").unwrap_or_else(String::new));
    let tags = tags.iter().fold(String::new(), |prefix, suffix| {
        prefix + &*urlencoding::encode(suffix.0.trim()) + "/"
    });
    Redirect::to(format!("/t/{}/{}", add_tag, tags).parse().unwrap())
}

#[derive(Template)]
#[template(path = "index.html")]
pub struct Index {
    tags:      Vec<Tag>,
    posts:     Vec<ThreadLink>,
    curr_path: String,
    offers:    i64,
}

#[derive(Serialize)]
struct ThreadLink {
    num:            usize,
    id:             i32,
    title:          String,
    date:           String,
    emphasize_date: bool,
    read:           bool,
    jump_to:        i32,
    replies:        String,
    tags:           Vec<String>,
}

impl Index {
    pub async fn show(user: User) -> Self {
        Self::show_with_tags(user, Path(String::from("en"))).await
    }

    pub async fn show_with_tags(user: User, Path(viewed_tags): Path<String>) -> Self {
        use self::threads::dsl::*;

        let viewed_tags = Tags::from(&*viewed_tags);
        let conn = crate::establish_db_connection();

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
                // Format the date:
                // TODO: Consider moving duration->plaintext into common utility
                let duration_since_last_post = Utc::now().naive_utc()
                    - Reply::fetch(&conn, thread.last_post).unwrap().post_date;
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

                // Count the number of replies:
                let num_replies = {
                    use self::replies::dsl::*;

                    replies
                        .filter(thread_id.eq(thread.id))
                        .count()
                        .get_result(&conn)
                        .unwrap_or(0)
                };

                let replies = match num_replies {
                    0 | 1 => format!("No replies"),
                    2 => format!("1 reply"),
                    x => format!("{} replies", x - 1),
                };

                let read = user.has_read(&conn, &thread);
                let jump_to = user.next_unread(&conn, &thread);

                ThreadLink {
                    num: i + 1,
                    id: thread.id,
                    title: thread.title,
                    date: duration_string,
                    emphasize_date: duration_min < MINUTES_TIMESTAMP_IS_EMPHASIZED,
                    read,
                    jump_to,
                    replies,
                    tags: thread
                        .tags
                        .into_iter()
                        .filter_map(|tid| Tag::fetch_from_id(&conn, tid))
                        .map(|t| t.name)
                        .collect(),
                }
            })
            .collect();

        Self {
            curr_path: viewed_tags.fmt(),
            tags:      viewed_tags.tags,
            posts:     posts,
            offers:    IncomingOffer::count(&conn, &user),
        }
    }
}

#[derive(Template)]
#[template(path = "thread.html")]
pub struct ThreadPage {
    id:     i32,
    title:  String,
    posts:  Vec<Post>,
    offers: i64,
}

#[derive(Serialize)]
struct Reward {
    thumbnail:   String,
    name:        String,
    description: String,
    rarity:      String,
}

#[derive(Serialize)]
struct Post {
    id:        i32,
    author:    ProfileStub,
    body:      String,
    body_html: String,
    date:      String,
    reactions: Vec<ItemThumbnail>,
    reward:    Option<Reward>,
    can_react: bool,
    can_edit:  bool,
    image:     Option<String>,
    thumbnail: Option<String>,
    filename:  String,
}

impl ThreadPage {
    pub async fn show(user: User, Path(thread_id): Path<i32>) -> Result<Self, crate::NotFound> {
        use self::threads::dsl::*;

        let conn = crate::establish_db_connection();
        let offers = user.incoming_offers(&conn);
        let thread = &threads
            .filter(id.eq(thread_id))
            .first::<Thread>(&conn)
            .map_err(|_| NotFound::new(offers))?;
        user.read_thread(&conn, &thread);

        let mut user_cache = UserCache::new(&conn);
        let posts = replies::dsl::replies
            .filter(replies::dsl::thread_id.eq(thread_id))
            .order(replies::dsl::post_date.asc())
            .load::<Reply>(&conn)
            .unwrap()
            .into_iter()
            .map(|t| Post {
                id:        t.id,
                author:    user_cache.get(t.author_id).clone(),
                body:      t.body,
                body_html: t.body_html,
                // TODO: we need to add a user setting to format this to the local time.
                date:      t.post_date.format(DATE_FMT).to_string(),
                reactions: t
                    .reactions
                    .into_iter()
                    .map(|d| ItemDrop::fetch(&conn, d).thumbnail(&conn))
                    .collect(),
                reward:    t.reward.map(|r| {
                    let drop = ItemDrop::fetch(&conn, r);
                    let item = drop.fetch_item(&conn);
                    Reward {
                        name:        item.name,
                        description: item.description,
                        thumbnail:   drop.thumbnail_html(&conn),
                        rarity:      item.rarity.to_string(),
                    }
                }),
                can_edit:  t.author_id == user.id, // TODO: Add time limit for replies
                can_react: t.author_id != user.id,
                image:     t.image,
                thumbnail: t.thumbnail,
                filename:  t.filename,
            })
            .collect::<Vec<_>>();

        Ok(Self {
            id: thread_id,
            title: thread.title.clone(), // I feel like I should be able to move this
            posts,
            offers: IncomingOffer::count(&conn, &user),
        })
    }
}

#[derive(Template, Debug)]
#[template(path = "author.html")]
pub struct AuthorPage {
    offers: i64,
}

impl AuthorPage {
    pub async fn show(user: User) -> Self {
        Self {
            offers: IncomingOffer::count(&crate::establish_db_connection(), &user),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ThreadForm {
    title: String,
    tags:  String,
    body:  String,
}

impl ThreadForm {
    pub async fn submit(
        user: User,
        form: Result<MultipartForm<Self, MAXIMUM_FILE_SIZE>, JsonError>,
    ) -> Result<Json<Thread>, JsonError> {
        let MultipartForm { file, form: thread } = form?;

        if thread.title.is_empty() || (thread.body.is_empty() && file.is_none()) {
            bail!("Thread title or body cannot be empty");
        }

        let post_date = Utc::now().naive_utc();
        let (image, thumbnail, filename) = upload_image(file).await?;

        // Parse the body as markdown
        let html_output = markdown_to_html(&thread.body.replace("\n", "\n\n"), &MARKDOWN_OPTIONS);

        let conn = crate::establish_db_connection();
        let tags = parse_tag_list(&thread.tags)
            .filter_map(|t| Tag::fetch_and_inc(&conn, t))
            .map(|t| t.id())
            .collect();

        conn.transaction(|| -> Result<Thread, diesel::result::Error> {
            use diesel::result::Error::RollbackTransaction;

            let next_thread = diesel::select(nextval(pg_get_serial_sequence("threads", "id")))
                .first::<i64>(&conn)
                .map_err(|_| RollbackTransaction)? as i32;

            let next_thread = next_thread as i32;

            let first_post: Reply = diesel::insert_into(replies::table)
                .values(&NewReply {
                    author_id: user.id,
                    thread_id: next_thread,
                    post_date,
                    body: &thread.body,
                    body_html: &html_output,
                    reward: ItemDrop::drop(&conn, &user).map(|d| d.id),
                    reactions: Vec::new(),
                    image,
                    thumbnail,
                    filename,
                })
                .get_result(&conn)
                .map_err(|_| RollbackTransaction)?;

            diesel::insert_into(threads::table)
                .values(&NewThread {
                    id: next_thread,
                    last_post: first_post.id,
                    title: &thread.title,
                    tags,
                })
                .get_result(&conn)
                .map_err(|_| RollbackTransaction)
        })
        .map_err(|_| error!("Unable to post thread. Try again later"))
        .map(Json)
    }
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
    pub id:         i32,
    pub name:       String,
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

    /// Fetches a tag, creating it if it doesn't already exist. num_tagged is
    /// incremented or set to one.
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

impl From<&'_ str> for Tags {
    fn from(path: &'_ str) -> Self {
        let conn = crate::establish_db_connection();
        let mut seen = HashSet::new();
        let tags = path
            .split("/")
            .filter_map(|s| {
                Tag::fetch_if_exists(&conn, s)
                    .map(|t| seen.insert(t.id).then(|| t))
                    .flatten()
            })
            .collect::<Vec<_>>();
        Tags { tags }
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
        body_html -> Text,
        reward -> Nullable<Integer>,
        reactions -> Array<Integer>,
        image -> Nullable<Text>,
        thumbnail -> Nullable<Text>,
        filename -> Text,
    }
}

#[derive(Queryable, Debug, Serialize)]
pub struct Reply {
    /// Id of the reply
    pub id:        i32,
    /// Id of the author
    pub author_id: i32,
    /// Id of the thread
    pub thread_id: i32,
    /// Date of posting
    #[serde(skip)] // TODO: Serialize this
    pub post_date: NaiveDateTime,
    /// Body of the reply
    pub body:      String,
    /// Body of the reply parsed to html (what the user typically sees)
    pub body_html: String,
    /// Any item that was rewarded for this post
    pub reward:    Option<i32>,
    /// Reactions attached to this post
    pub reactions: Vec<i32>,
    /// Image associated with this post
    pub image:     Option<String>,
    /// Thumbnail associated with this post's image
    pub thumbnail: Option<String>,
    /// Filename associated with the image
    pub filename:  String,
}

impl Reply {
    pub fn fetch(conn: &PgConnection, reply_id: i32) -> Result<Self, diesel::result::Error> {
        use self::replies::dsl::*;

        replies.filter(id.eq(reply_id)).first::<Reply>(conn)
    }
}

#[derive(Insertable, Debug)]
#[table_name = "replies"]
pub struct NewReply<'b, 'h> {
    author_id: i32,
    thread_id: i32,
    post_date: NaiveDateTime,
    body:      &'b str,
    body_html: &'h str,
    reward:    Option<i32>,
    reactions: Vec<i32>,
    image:     Option<String>,
    thumbnail: Option<String>,
    filename:  String,
}

#[derive(Deserialize)]
pub struct ReplyForm {
    reply: String,
}

impl ReplyForm {
    pub async fn submit(
        user: User,
        Path(thread_id): Path<i32>,
        MultipartForm {
            file,
            form: ReplyForm { reply },
        }: MultipartForm<Self, MAXIMUM_FILE_SIZE>,
    ) -> Result<Json<Reply>, JsonError> {
        let reply = reply.trim();
        if reply.is_empty() && file.is_none() {
            bail!("Reply cannot be empty");
        }

        let post_date = Utc::now().naive_utc();
        let (image, thumbnail, filename) = upload_image(file).await?;

        let conn = crate::establish_db_connection();
        let html_output = parse_post(&conn, reply, thread_id);

        let reply: Reply = diesel::insert_into(replies::table)
            .values(&NewReply {
                author_id: user.id,
                thread_id,
                post_date,
                body: &reply,
                body_html: &html_output,
                reward: ItemDrop::drop(&conn, &user).map(|d| d.id),
                reactions: Vec::new(),
                image,
                thumbnail,
                filename,
            })
            .get_result(&conn)?;

        let _: Thread = diesel::update(threads::table)
            .filter(threads::dsl::id.eq(thread_id))
            .set(threads::dsl::last_post.eq(reply.id))
            .get_result(&conn)?;

        Ok(Json(reply))
    }
}

#[derive(Deserialize)]
pub struct EditPostForm {
    // TODO: Name every post contents field "body"
    body: String,
}

impl EditPostForm {
    pub async fn submit(
        user: User,
        Path(post_id): Path<i32>,
        Form(EditPostForm { body }): Form<EditPostForm>,
    ) -> Result<Json<Reply>, JsonError> {
        let body = body.trim();

        let conn = crate::establish_db_connection();
        let post = Reply::fetch(&conn, post_id)?;

        if post.author_id != user.id {
            bail!("Cannot edit post you do not own");
        }

        if post.image.is_none() && body.is_empty() {
            bail!("Cannot edit a post to make it empty");
        }

        // TODO: check if time period to edit has expired.
        let html_output = parse_post(&conn, body, post.thread_id)
            + &format!(
                r#" <span style="font-size: 80%; color: grey">Edited on {}</span>"#,
                Utc::now().naive_utc().format(DATE_FMT)
            );

        Ok(diesel::update(replies::table)
            .filter(replies::dsl::id.eq(post_id))
            .set((
                replies::dsl::body.eq(body),
                replies::dsl::body_html.eq(html_output),
            ))
            .get_result(&conn)
            .map(Json)?)
    }
}

use comrak::{markdown_to_html, ComrakOptions};

lazy_static! {
    static ref MARKDOWN_OPTIONS: ComrakOptions = {
        let mut options = ComrakOptions::default();
        options.extension.strikethrough = true;
        options.extension.table = true;
        options.extension.autolink = true;
        options.extension.tasklist = true;
        options.render.hardbreaks = true;
        options.render.escape = true;
        options.parse.smart = true;
        options
    };
}

// TODO: This probably needs to be async
fn parse_post(conn: &PgConnection, body: &str, thread_id: i32) -> String {
    lazy_static! {
        static ref REPLY_RE: Regex = Regex::new(r"@(?P<reply_id>\d*)").unwrap();
    }

    let referenced_reply_ids = REPLY_RE
        .captures_iter(&body)
        .map(|captured_group| captured_group["reply_id"].to_string())
        .collect::<Vec<String>>();

    let mut user_cache = UserCache::new(conn);
    let id_to_author = replies::dsl::replies
        .filter(replies::dsl::thread_id.eq(thread_id))
        .order(replies::dsl::post_date.asc())
        .load::<Reply>(conn)
        .unwrap()
        .into_iter()
        .filter(|reply| referenced_reply_ids.contains(&reply.id.to_string()))
        .map(|reply| {
            (
                reply.id.to_string(),
                user_cache.get(reply.author_id).clone().name,
            )
        })
        .collect::<HashMap<String, String>>();

    let response_divs = replies::dsl::replies
        .filter(replies::dsl::thread_id.eq(thread_id))
        .order(replies::dsl::post_date.asc())
        .load::<Reply>(conn)
        .unwrap()
        .into_iter()
        .filter(|reply| referenced_reply_ids.contains(&reply.id.to_string())).map(|reply| {
            format!(
                r#"<div class="respond-to-preview action-box" reply_id={reply_id}><b>@{author_name}</b></div><div class="overlay-on-hover reply-overlay"></div>"#,
                reply_id = reply.id.to_string(),
                author_name = user_cache.get(reply.author_id).clone().name,
            )
        })
        .collect::<String>();

    // TODO: replace markdown parser with our own
    // Parse the body as markdown
    let html_output =
        response_divs + &markdown_to_html(&body.replace("\n", "\n\n"), &MARKDOWN_OPTIONS);

    // Swap out "respond" command sequences for @ mentions
    REPLY_RE.replace_all(&html_output, |captured_group: &Captures| {
            let reply_id = &captured_group["reply_id"];
            if id_to_author.contains_key(reply_id) {
                format!(
                    r#"<span class="respond-to-preview" reply_id={reply_id}><b>@{author_name}</b></span><div class="overlay-on-hover reply-overlay"></div>"#,
                    reply_id = reply_id,
                    author_name = id_to_author[reply_id],
                )
            } else {
                captured_group[0].to_string()
            }
        }).to_string()
}

use std::io::Cursor;

use aws_sdk_s3::{
    error::PutObjectError,
    model::ObjectCannedAcl,
    output::PutObjectOutput,
    types::{ByteStream, SdkError},
    Client, Endpoint,
};
use base64ct::{Base64Url, Encoding};
use image::ImageFormat;
use sha2::{Digest, Sha256};
use tokio::task;

pub struct Image {
    pub filename:  String,
    pub thumbnail: Option<String>,
}

pub const IMAGE_STORE_ENDPOINT: &'static str = "https://marche-storage.nyc3.digitaloceanspaces.com";
pub const IMAGE_STORE_BUCKET: &'static str = "images";

pub fn get_url(filename: &str) -> String {
    format!("{IMAGE_STORE_ENDPOINT}/{IMAGE_STORE_BUCKET}/{filename}")
}

pub const MAXIMUM_FILE_SIZE: u64 = 12 * 1024 * 1024; /* 12mb */

async fn image_exists(client: &Client, filename: &str) -> bool {
    client
        .head_object()
        .bucket(IMAGE_STORE_BUCKET)
        .key(filename)
        .send()
        .await
        .is_ok()
}

async fn put_image(
    client: &Client,
    filename: &str,
    ext: &str,
    body: ByteStream,
) -> Result<PutObjectOutput, SdkError<PutObjectError>> {
    client
        .put_object()
        .acl(ObjectCannedAcl::PublicRead)
        .content_type(format!("image/{}", ext))
        .bucket(IMAGE_STORE_BUCKET)
        .key(filename)
        .body(body)
        .send()
        .await
}

// TODO: move return type to struct
async fn upload_image(
    file: Option<File>,
) -> Result<(Option<String>, Option<String>, String), JsonError> {
    match file {
        Some(file) => {
            let Image {
                filename,
                thumbnail,
            } = upload_bytes(file.bytes).await?;
            Ok((Some(filename), thumbnail, file.name))
        }
        None => Ok((None, None, String::new())),
    }
}

/// Upload image to object storage
async fn upload_bytes(bytes: Bytes) -> Result<Image, JsonError> {
    /// Maximum width/height of an image.
    const MAX_WH: u32 = 400;

    let format = image::guess_format(&bytes)?;
    let ext = match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpeg",
        ImageFormat::Gif => "gif",
        ImageFormat::WebP => "webp",
        _ => bail!("Invalid extension"),
    };

    let (bytes, hash) = task::spawn_blocking(move || {
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        (bytes, Base64Url::encode_string(&hasher.finalize()))
    })
    .await?;

    // Check if file already exists:
    let config = aws_config::from_env()
        .endpoint_resolver(Endpoint::immutable(
            IMAGE_STORE_ENDPOINT.parse().expect("valid URI"),
        ))
        .load()
        .await;
    let client = Client::new(&config);
    let filename = format!("{hash}.{ext}");

    if image_exists(&client, &filename).await {
        let thumbnail = format!("{hash}_thumbnail.{ext}");
        return Ok(Image {
            filename:  get_url(&filename),
            thumbnail: image_exists(&client, &thumbnail)
                .await
                .then(move || get_url(&thumbnail)),
        });
    }

    // Resize the image if it is necessary
    let image = image::load_from_memory(&bytes)?;
    let thumbnail = if image.height() > MAX_WH || image.width() > MAX_WH {
        let thumb = task::spawn_blocking(move || image.thumbnail(MAX_WH, MAX_WH)).await?;
        let mut output = Cursor::new(Vec::with_capacity(thumb.as_bytes().len()));
        thumb.write_to(&mut output, format)?;
        let thumbnail = format!("{hash}_thumbnail.{ext}");
        put_image(
            &client,
            &thumbnail,
            &ext,
            ByteStream::from(output.into_inner()),
        )
        .await?;
        Some(thumbnail)
    } else {
        None
    };

    put_image(&client, &filename, &ext, ByteStream::from(bytes)).await?;

    Ok(Image {
        filename:  get_url(&filename),
        thumbnail: thumbnail.as_deref().map(get_url),
    })
}
