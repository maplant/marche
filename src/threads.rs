//! Display threads
use std::collections::{HashMap, HashSet};

use axum::{
    body::Bytes,
    extract::{Extension, Form, Path, Query},
    Json,
};
use chrono::{prelude::*, NaiveDateTime};
use derive_more::From;
use futures::stream::StreamExt;
use lazy_static::lazy_static;
use marche_proc_macros::json_result;
use regex::{Captures, Regex};
use serde::{Deserialize, Serialize};
use sqlx::{Connection, FromRow, PgExecutor, PgPool};

use crate::{
    items::{Item, ItemDrop, ItemType},
    post,
    users::{Role, User /*, UserCache*/},
    File, MultipartForm, MultipartFormError,
};

#[derive(FromRow, Default, Debug, Serialize)]
pub struct Thread {
    /// Id of the thread
    pub id: i32,
    /// Id of the last post
    pub last_post: i32,
    /// Title of the thread
    pub title: String,
    /// Tags given to this thread
    pub tags: Vec<i32>,
    /// Number of replies to this thread, not including the first.
    pub num_replies: i32,
    /// Whether or not the thread is pinned
    pub pinned: bool,
    /// Whether or not the thread is locked
    pub locked: bool,
    /// Whether or not the thread is hidden
    pub hidden: bool,
}

impl Thread {
    pub async fn fetch(conn: &PgPool, id: i32) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM threads WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await
    }
}

/*
post! {
    "/delete_thread/:dead_thread_id",
    #[json_result]
    pub async fn delete_thread(
        pool: Extension<PgPool>,
        user: User,
        Path(dead_thread_id): Path<i32>,
    ) -> Json<Result<(), DeleteThreadError>> {
        if user.role < Role::Moderator {
            return Err(DeleteThreadError::Unprivileged);
        }

        let conn = pool.get().expect("Could not connect to db");

        // Fetch the thread title for logging purposes
        let thread_title = Thread::fetch(&conn, dead_thread_id)
            .map_err(|_| DeleteThreadError::NoSuchThread)?
            .title;

        let _ = conn.transaction(|| -> Result<(), diesel::result::Error> {
            // Delete the thread:
            {
                use crate::schema::threads::dsl::*;
                diesel::delete(threads.filter(id.eq(dead_thread_id))).execute(&mut conn)?;
            }
            // Delete any reply on the thread:
            {
                use crate::schema::replies::dsl::*;
                diesel::delete(replies.filter(thread_id.eq(dead_thread_id)))
                    .execute(&mut conn)
                    .map_err(|_| diesel::result::Error::RollbackTransaction)?;
            }

            Ok(())
        });

        tracing::info!(
            "User `{}` has deleted thread {dead_thread_id} titled: `{thread_title}`",
            user.name
        );

        Ok(())
    }
}
*/

#[derive(Debug, Deserialize)]
pub struct ThreadForm {
    title: String,
    tags: String,
    body: String,
}

#[derive(Serialize, From)]
pub enum SubmitThreadError {
    TitleOrBodyIsEmpty,
    TagTooLong,
    TooManyTags,
    UploadImageError(UploadImageError),
    InternalDbError(#[serde(skip)] sqlx::Error),
    MultipartFormError(MultipartFormError),
}

pub const MAX_TAG_LEN: usize = 16;
pub const MAX_NUM_TAGS: usize = 6;

post! {
    "/thread",
    #[json_result]
    async fn new_thread(
        conn: Extension<PgPool>,
        user: User,
        form: Result<MultipartForm<ThreadForm, MAXIMUM_FILE_SIZE>, MultipartFormError>,
    ) -> Json<Result<Thread, SubmitThreadError>> {
        let MultipartForm { file, form: thread } = form?;

        let title = thread.title.trim();
        let body = thread.body.trim();

        if title.is_empty() || (body.is_empty() && file.is_none()) {
            return Err(SubmitThreadError::TitleOrBodyIsEmpty);
        }

        let post_date = Utc::now().naive_utc();
        let (image, thumbnail, filename) = upload_image(file).await?;

        let mut tags = Vec::new();
        for tag in parse_tag_list(&thread.tags) {
            let tag = tag.trim();
            if tag.is_empty() {
                continue;
            }
            if tag.len() > MAX_TAG_LEN {
                return Err(SubmitThreadError::TagTooLong);
            }
            tags.push(tag);
        }

        if tags.len() > MAX_NUM_TAGS {
            return Err(SubmitThreadError::TooManyTags);
        }

        let mut transaction = conn.begin().await?;

        let mut tag_ids = Vec::new();
        for tag in tags.into_iter() {
            if let Some(tag) = Tag::fetch_from_str_and_inc(&mut *transaction, tag).await? {
                tag_ids.push(tag.id());
            }
        }


        let thread: Thread = sqlx::query_as(
            r#"
                 INSERT INTO threads
                     (title, tags, last_post, num_replies, pinned, locked, hidden)
                  VALUES
                     ($1, $2, 0, 0, FALSE, FALSE, FALSE)
            "#,
        )
        .bind(title)
        .bind(tag_ids)
        .fetch_one(&mut *transaction)
        .await?;

        let item_drop = ItemDrop::drop(&mut transaction, &user)
            .await?
            .map(ItemDrop::to_id);

        let reply: Reply = sqlx::query_as(
            r#"
                 INSERT INTO replies
                     (author_id, thread_id, post_date, body, reward, image, thumbnail, filename, reactions)
                 VALUES
                     ($1, $2, $3, $4, $5, $6, $7, $8, [])
            "#
        )
        .bind(user.id)
        .bind(thread.id)
        .bind(post_date)
        .bind(body)
        .bind(item_drop)
        .bind(image)
        .bind(thumbnail)
        .bind(filename)
        .fetch_one(&mut *transaction)
        .await?;

        let thread = sqlx::query_as("UPDATE threads SET last_post = $1 WHERE id = $2")
            .bind(reply.id)
            .bind(thread.id)
            .fetch_one(&mut *transaction)
            .await?;

        transaction.commit().await?;

        Ok(thread)
    }
}

#[derive(Deserialize)]
struct UpdateThread {
    locked: Option<bool>,
    pinned: Option<bool>,
    hidden: Option<bool>,
}

#[derive(Serialize, From)]
enum UpdateThreadError {
    Unprivileged,
    InternalDbError(#[serde(skip)] sqlx::Error),
}

/*
post! {
    "/thread/:thread_id",
    #[json_result]
    async fn update_thread_flags(
        pool: Extension<PgPool>,
        user: User,
        Path(thread_id): Path<i32>,
        Query(UpdateThread {
            locked: set_locked,
            pinned: set_pinned,
            hidden: set_hidden,
        }): Query<UpdateThread>
    ) -> Json<Result<(), UpdateThreadError>> {
        use crate::schema::threads::dsl::*;

        if user.role < Role::Moderator {
            return Err(UpdateThreadError::Unprivileged);
        }

        if set_locked.is_none() && set_pinned.is_none() && set_hidden.is_none() {
            return Ok(());
        }

        let conn = pool.get().expect("Could not connect to db");

        // TODO: Come up with some pattern to chain these

        if let Some(set_locked) = set_locked {
            diesel::update(threads.find(thread_id))
                .set(locked.eq(set_locked))
                .get_result::<Thread>(&conn)?;
         }

        if let Some(set_pinned) = set_pinned {
            diesel::update(threads.find(thread_id))
                .set(pinned.eq(set_pinned))
                .get_result::<Thread>(&conn)?;
        }

        if let Some(set_hidden) = set_hidden {
            diesel::update(threads.find(thread_id))
                .set(hidden.eq(set_hidden))
                .get_result::<Thread>(&conn)?;
        }

        Ok(())
    }
}
*/

#[derive(Serialize, From)]
pub enum DeleteThreadError {
    Unprivileged,
    NoSuchThread,
    InternalDbError(#[serde(skip)] sqlx::Error),
}

#[derive(Debug, FromRow, Serialize, Clone)]
pub struct Tag {
    pub id: i32,
    pub name: String,
    /// Number of posts that have been tagged with this tag.
    pub num_tagged: i32,
}

impl Tag {
    pub fn id(&self) -> i32 {
        self.id
    }

    /// Returns the most popular tags.
    pub async fn popular(conn: &PgPool) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM tags ORDER BY num_tagged DESC LIMIT 10")
            .fetch_all(conn)
            .await
    }

    pub async fn fetch_from_id(conn: &PgPool, id: i32) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM tags WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await
    }

    pub async fn fetch_from_str(conn: &PgPool, tag: &str) -> Result<Option<Self>, sqlx::Error> {
        let tag_name = clean_tag_name(tag);

        if tag_name.is_empty() {
            return Ok(None);
        }

        sqlx::query_as("SELECT * FROM tags WHERE name = $1")
            .bind(tag_name)
            .fetch_optional(conn)
            .await
    }

    /// Fetches a tag, creating it if it doesn't already exist. num_tagged is
    /// incremented or set to one.
    ///
    /// It's kind of a weird interface, I'm open to suggestions.
    ///
    /// Assumes that str is not empty.
    pub async fn fetch_from_str_and_inc(
        conn: impl PgExecutor<'_>,
        tag: &str,
    ) -> Result<Option<Self>, sqlx::Error> {
        let tag_name = clean_tag_name(tag);

        if tag_name.is_empty() {
            return Ok(None);
        }

        sqlx::query_as(
            r#"
                INSERT INTO tags (name)
                VALUES ($1)
                ON CONFLICT (name) DO UPDATE SET num_tagged = num_tagged + 1
                RETURNING *
            "#,
        )
        .bind(tag_name)
        .fetch_optional(conn)
        .await
    }
}

fn clean_tag_name(name: &str) -> String {
    name.trim().to_lowercase()
}

fn parse_tag_list(list: &str) -> impl Iterator<Item = &str> {
    // TODO: More stuff!
    list.split(",").map(|i| i.trim())
}

#[derive(Debug, Clone)]
pub struct Tags {
    pub tags: Vec<Tag>,
}

impl Tags {
    pub async fn fetch_from_str(conn: &PgPool, path: &str) -> Self {
        let mut seen = HashSet::new();
        let tags = futures::stream::iter(path.split("/"))
            .filter_map(move |s| async move { Tag::fetch_from_str(conn, s).await.ok().flatten() })
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .filter(move |t| seen.insert(t.id))
            .collect();
        Tags { tags }
    }

    pub async fn fetch_from_ids<'a>(conn: &PgPool, ids: impl Iterator<Item = &'a i32>) -> Self {
        Self {
            tags: futures::stream::iter(ids)
                .filter_map(|id| async move { Tag::fetch_from_id(conn, *id).await.ok().flatten() })
                .collect()
                .await,
        }
    }

    // Not going to deal with lifetimes here. Just clone it.

    pub fn into_names(self) -> impl Iterator<Item = String> {
        self.tags.into_iter().map(|x| x.name)
    }

    pub fn into_ids(self) -> impl Iterator<Item = i32> {
        self.tags.into_iter().map(|x| x.id)
    }

    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }
}

#[derive(FromRow, Debug, Serialize)]
pub struct Reply {
    /// Id of the reply
    pub id: i32,
    /// Id of the author
    pub author_id: i32,
    /// Id of the thread
    pub thread_id: i32,
    /// Date of posting
    #[serde(skip)] // TODO: Serialize this
    pub post_date: NaiveDateTime,
    /// Body of the reply
    pub body: String,
    /// Any item that was rewarded for this post
    pub reward: Option<i32>,
    /// Reactions attached to this post
    pub reactions: Vec<i32>,
    /// Image associated with this post
    pub image: Option<String>,
    /// Thumbnail associated with this post's image
    pub thumbnail: Option<String>,
    /// Filename associated with the image
    pub filename: String,
    /// Whether or not the thread is hidden
    pub hidden: bool,
}

impl Reply {
    pub async fn fetch(conn: &PgPool, id: i32) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM replies WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await
    }
}

#[derive(Serialize, From)]
pub enum DeleteReplyError {
    Unprivileged,
    NoSuchReply,
    CannotDeleteFirstReply,
    InternalDbError(#[serde(skip)] sqlx::Error),
}

/*
post! {
    "/delete_reply/:dead_reply_id",
    #[json_result]
    async fn delete_reply(
        pool: Extension<PgPool>,
        user: User,
        Path(dead_reply_id): Path<i32>,
    ) -> Json<Result<(), DeleteReplyError>> {
        use crate::schema::replies::dsl::*;

        if user.role < Role::Moderator {
            return Err(DeleteReplyError::Unprivileged);
        }

        let conn = pool.get().expect("Could not connect to db");
        let dead_reply =
            Reply::fetch(&conn, dead_reply_id).map_err(|_| DeleteReplyError::NoSuchReply)?;

        // Get the post before this one in case last_post is the dead reply
        let prev_reply = replies
            .filter(thread_id.eq(dead_reply.thread_id))
            .filter(id.lt(dead_reply_id))
            .order(id.desc())
            .first::<Reply>(&conn)
            .map_err(|_| DeleteReplyError::CannotDeleteFirstReply)?;

        {
            use crate::schema::threads::dsl::*;

            // Discard any error:
            let _ = diesel::update(threads.find(dead_reply.thread_id))
                .filter(last_post.eq(dead_reply_id))
                .set(last_post.eq(prev_reply.id))
                .get_result::<Thread>(&conn);

            // Reduce the number of replies by one:
            diesel::update(threads.find(dead_reply.thread_id))
                .set(num_replies.eq(num_replies - 1))
                .get_result::<Thread>(&conn)?;
        }

        diesel::delete(replies.find(dead_reply_id)).execute(&conn)?;

        tracing::info!(
            "User `{}` has deleted reply {dead_reply_id} in thread {}",
            user.name,
            dead_reply.thread_id,
        );

        Ok(())
    }
}
 */

#[derive(Deserialize)]
pub struct ReplyForm {
    body: String,
    thread_id: String,
}

#[derive(Serialize, From)]
pub enum ReplyError {
    NoSuchThread,
    ReplyIsEmpty,
    ThreadIsLocked,
    UploadImageError(#[serde(skip)] UploadImageError),
    InternalDbError(#[serde(skip)] sqlx::Error),
}

post! {
    "/reply",
    #[json_result]
    pub async fn new_reply(
        conn: Extension<PgPool>,
        user: User,
        MultipartForm {
            file,
            form: ReplyForm { thread_id, body },
        }: MultipartForm<ReplyForm, MAXIMUM_FILE_SIZE>,
    ) -> Json<Result<Reply, ReplyError>> {
        let body = body.trim();

        if body.is_empty() && file.is_none() {
            return Err(ReplyError::ReplyIsEmpty);
        }

        let thread_id: i32 = thread_id.parse().map_err(|_| ReplyError::NoSuchThread)?;
        if Thread::fetch(&conn, thread_id)
            .await?
            .ok_or(ReplyError::NoSuchThread)?
            .locked
        {
            return Err(ReplyError::ThreadIsLocked);
        }

        let post_date = Utc::now().naive_utc();
        let (image, thumbnail, filename) = upload_image(file).await?;

        let mut transaction = conn.begin().await?;

        let reply: Reply = sqlx::query_as(
            r#"
                INSERT INTO replies
                    (author_id, thread_id, post_date, body, reward, image, thumbnail, filename, reactions)
                VALUES
                    ($1, $2, $3, $4, $5, $6, $7, $8, [])
            "#
        )
        .bind(user.id)
        .bind(thread_id)
        .bind(post_date)
        .bind(body)
        .bind(
            ItemDrop::drop(&mut transaction, &user)
                .await?
                .map(ItemDrop::to_id)
        )
        .bind(image)
        .bind(thumbnail)
        .bind(filename)
        .fetch_one(&mut *transaction)
        .await?;

        sqlx::query("UPDATE threads SET last_post = $1, num_replies = num_replies + 1")
            .bind(reply.id)
            .execute(&mut *transaction)
            .await?;

        Ok(reply)
    }
}

/*
#[derive(Deserialize)]
pub struct UpdateReplyParams {
    hidden: Option<bool>,
}

#[derive(Deserialize)]
pub struct UpdateReplyForm {
    body: Option<String>,
}

#[derive(Serialize, From)]
pub enum UpdateReplyError {
    Unprivileged,
    NotYourPost,
    CannotMakeEmpty,
    InternalDbError(#[serde(skip)] sqlx::Error),
}

post! {
    "/reply/:post_id",
    #[json_result]
    pub async fn update_reply(
        pool: Extension<PgPool>,
        user: User,
        Path(post_id): Path<i32>,
        Query(UpdateReplyParams {
            hidden,
        }): Query<UpdateReplyParams>,
        Form(UpdateReplyForm { body }): Form<UpdateReplyForm>,
    ) -> Json<Result<Reply, UpdateReplyError>> {
        let conn = pool.get().expect("Could not connect to db");
        let post = Reply::fetch(&conn, post_id)?;

        let reply = if let Some(hidden) = hidden {
            use crate::schema::replies::dsl::*;

            if user.role < Role::Moderator {
                return Err(UpdateReplyError::Unprivileged);
            }

            Some(diesel::update(replies.find(post_id))
                .set(hidden.eq(hidden))
                .get_result::<Reply>(&conn)?)
        } else {
            None
        };

        let body = if let Some(body) = body {
            body
        } else {
            return Ok(reply.unwrap_or(post));
        };

        if post.author_id != user.id && user.role < Role::Moderator {
            return Err(UpdateReplyError::NotYourPost);
        }

        let body = body.trim();

        if post.image.is_none() && body.is_empty() {
            return Err(UpdateReplyError::CannotMakeEmpty);
        }

        // TODO: check if time period to edit has expired.
        let html_output = parse_post(&conn, body, post.thread_id)
            + &format!(
                r#" <span style="font-size: 80%; color: grey">Edited on {}</span>"#,
                Utc::now().naive_utc().format(crate::DATE_FMT)
            );

        Ok({
            use crate::schema::replies::dsl::*;

            diesel::update(replies.find(post_id))
                .set((
                    body.eq(body),
                    body_html.eq(html_output),
                ))
                .get_result::<Reply>(&conn)?
        })
    }
}

#[derive(Serialize, From)]
pub enum ReactError {
    NoSuchReply,
    ThisIsYourPost,
    InternalDbError(#[serde(skip)] sqlx::Error),
}

post! {
    "/react/:post_id",
    #[json_result]
    pub async fn react(
        pool: Extension<PgPool>,
        user: User,
        Path(post_id): Path<i32>,
        Form(used_reactions): Form<HashMap<i32, String>>,
    ) -> Json<Result<(), ReactError>> {
        use diesel::result::Error;
        use crate::schema::replies::dsl::*;

        let conn = pool.get().expect("Could not connect to db");
        let reply = Reply::fetch(&conn, post_id).map_err(|_| ReactError::NoSuchReply)?;

        if reply.author_id == user.id {
            return Err(ReactError::ThisIsYourPost);
        }

        conn.transaction(|| -> Result<i32, Error> {
            let mut new_reactions = reply.reactions;

            let author =
                User::fetch(&conn, reply.author_id).map_err(|_| Error::RollbackTransaction)?;

            // Verify that all of the reactions are owned by the user:
            for (reaction, selected) in used_reactions.into_iter() {
                let drop = ItemDrop::fetch(&conn, reaction)
                    .map_err(|_| Error::RollbackTransaction)?;
                let item = Item::fetch(&conn, drop.item_id);
                if selected != "on" || drop.owner_id != user.id || !item.is_reaction() {
                    return Err(Error::RollbackTransaction);
                }

                // Set the drops to consumed.
                use crate::schema::drops::dsl::*;

                diesel::update(drops.find(reaction))
                    .filter(consumed.eq(false))
                    .set(consumed.eq(true))
                    .get_result::<ItemDrop>(&conn)
                    .map_err(|_| Error::RollbackTransaction)?;

                new_reactions.push(reaction);
                match item.item_type {
                    ItemType::Reaction { xp_value, .. } => {
                        author.add_experience(&conn, xp_value as i64)
                    }
                    _ => unreachable!(),
                }
            }

            // Update the post with the new reactions:
            // TODO: Move into Reply struct
            diesel::update(replies.find(post_id))
                .set(reactions.eq(new_reactions))
                .get_result::<Reply>(&conn)
                .map_err(|_| Error::RollbackTransaction)?;

            Ok(reply.thread_id)
        })?;

        Ok(())
    }
}
*/

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

/*
// TODO: This probably needs to be async
fn parse_post(conn: &PgConnection, body: &str, thread_id: i32) -> String {
    use crate::schema::replies::dsl::*;

    lazy_static! {
        static ref REPLY_RE: Regex = Regex::new(r"@(?P<reply_id>\d*)").unwrap();
    }

    let referenced_reply_ids = REPLY_RE
        .captures_iter(&body)
        .map(|captured_group| captured_group["reply_id"].to_string())
        .collect::<Vec<String>>();

    let mut user_cache = UserCache::new(conn);
    let id_to_author = replies
        .filter(thread_id.eq(thread_id))
        .order(post_date.asc())
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

    let response_divs = replies
        .filter(thread_id.eq(thread_id))
        .order(post_date.asc())
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
*/

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
    pub filename: String,
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

/*
pub struct ImageUploadResult {

}
*/

#[derive(Debug, Serialize, From)]
pub enum UploadImageError {
    InvalidExtension,
    ImageError(#[serde(skip)] image::ImageError),
    InternalServerError(#[serde(skip)] tokio::task::JoinError),
    InternalBlockStorageError(#[serde(skip)] SdkError<PutObjectError>),
}

// TODO: move return type to struct
async fn upload_image(
    file: Option<File>,
) -> Result<(Option<String>, Option<String>, String), UploadImageError> {
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
async fn upload_bytes(bytes: Bytes) -> Result<Image, UploadImageError> {
    /// Maximum width/height of an image.
    const MAX_WH: u32 = 400;

    let format = image::guess_format(&bytes)?;
    let ext = match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpeg",
        ImageFormat::Gif => "gif",
        ImageFormat::WebP => "webp",
        _ => return Err(UploadImageError::InvalidExtension),
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
            filename: get_url(&filename),
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
        filename: get_url(&filename),
        thumbnail: thumbnail.as_deref().map(get_url),
    })
}
