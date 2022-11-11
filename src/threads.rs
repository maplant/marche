//! Display threads
use std::collections::{HashMap, HashSet};

use axum::{
    body::Bytes,
    extract::{Extension, Form, Path, Query},
};
use chrono::{prelude::*, NaiveDateTime};
use futures::stream::StreamExt;
use marche_proc_macros::json_result;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgExecutor, PgPool};
use thiserror::Error;

use crate::{
    items::ItemDrop,
    post,
    users::{Role, User},
    File, MultipartForm, MultipartFormError,
};

#[derive(FromRow, Default, Debug, Serialize)]
pub struct Thread {
    /// Id of the thread
    pub id:          i32,
    /// Id of the last post
    pub last_post:   i32,
    /// Title of the thread
    pub title:       String,
    /// Tags given to this thread
    pub tags:        Vec<i32>,
    /// Number of replies to this thread, not including the first.
    pub num_replies: i32,
    /// Whether or not the thread is pinned
    pub pinned:      bool,
    /// Whether or not the thread is locked
    pub locked:      bool,
    /// Whether or not the thread is hidden
    pub hidden:      bool,
}

impl Thread {
    pub async fn fetch(conn: &PgPool, id: i32) -> Result<Self, sqlx::Error> {
        sqlx::query_as("SELECT * FROM threads WHERE id = $1")
            .bind(id)
            .fetch_one(conn)
            .await
    }

    pub async fn fetch_optional(conn: &PgPool, id: i32) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM threads WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await
    }
}

#[derive(Error, Serialize, Debug)]
pub enum DeleteThreadError {
    #[error("You are not privileged enough")]
    Unprivileged,
    #[error("No such thread exists")]
    NoSuchThread,
    #[error("Internal database error {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
}

post!(
    "/delete_thread/:dead_thread_id",
    #[json_result]
    pub async fn delete_thread(
        conn: Extension<PgPool>,
        user: User,
        Path(dead_thread_id): Path<i32>,
    ) -> Json<Result<(), DeleteThreadError>> {
        if user.role < Role::Moderator {
            return Err(DeleteThreadError::Unprivileged);
        }

        // Fetch the thread title for logging purposes
        let thread_title = Thread::fetch_optional(&*conn, dead_thread_id)
            .await?
            .ok_or(DeleteThreadError::NoSuchThread)?
            .title;

        let mut transaction = conn.begin().await?;

        // Delete the thread:
        sqlx::query("DELETE FROM threads WHERE id = $1")
            .bind(dead_thread_id)
            .execute(&mut transaction)
            .await?;

        // Delete all replies to the thread:
        sqlx::query("DELETE FROM replies WHERE thread_id = $1")
            .bind(dead_thread_id)
            .execute(&mut transaction)
            .await?;

        tracing::info!(
            "User `{}` has deleted thread {dead_thread_id} titled: `{thread_title}`",
            user.name
        );

        Ok(())
    }
);

#[derive(Debug, Deserialize)]
pub struct ThreadForm {
    title: String,
    tags:  String,
    body:  String,
}

#[derive(Debug, Serialize, Error)]
pub enum SubmitThreadError {
    #[error("title or body is empty")]
    TitleOrBodyIsEmpty,
    #[error("there is a tag that exceeds the maximum length ({MAX_TAG_LEN} characters")]
    TagTooLong,
    #[error("there are too many tags (maximum {MAX_NUM_TAGS} allowed)")]
    TooManyTags,
    #[error("error uploading image {0}")]
    UploadImageError(#[from] UploadImageError),
    #[error("internal db error {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
    #[error("multipart form error {0}")]
    MultipartFormError(#[from] MultipartFormError),
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
                 RETURNING *
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
                     ($1, $2, $3, $4, $5, $6, $7, $8, '{}')
                 RETURNING *
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

        let thread = sqlx::query_as("UPDATE threads SET last_post = $1 WHERE id = $2 RETURNING *")
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

#[derive(Serialize, Error, Debug)]
enum UpdateThreadError {
    #[error("You are not privileged enough")]
    Unprivileged,
    #[error("Internal database error: {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
}

post!(
    "/thread/:thread_id",
    #[json_result]
    async fn update_thread_flags(
        conn: Extension<PgPool>,
        user: User,
        Path(thread_id): Path<i32>,
        Query(UpdateThread {
            locked,
            pinned,
            hidden,
        }): Query<UpdateThread>,
    ) -> Json<Result<(), UpdateThreadError>> {
        if user.role < Role::Moderator {
            return Err(UpdateThreadError::Unprivileged);
        }

        if locked.is_none() && pinned.is_none() && hidden.is_none() {
            return Ok(());
        }

        // TODO: Come up with some pattern to chain these

        if let Some(locked) = locked {
            sqlx::query("UPDATE threads SET locked = $1 WHERE id = $2")
                .bind(locked)
                .bind(thread_id)
                .execute(&*conn)
                .await?;
        }

        if let Some(pinned) = pinned {
            sqlx::query("UPDATE threads SET pinned = $1 WHERE id = $2")
                .bind(pinned)
                .bind(thread_id)
                .execute(&*conn)
                .await?;
        }

        if let Some(hidden) = hidden {
            sqlx::query("UPDATE threads SET hidden = $1 WHERE id = $2")
                .bind(hidden)
                .bind(thread_id)
                .execute(&*conn)
                .await?;
        }

        Ok(())
    }
);

#[derive(Debug, FromRow, Serialize, Clone)]
pub struct Tag {
    pub id:         i32,
    pub name:       String,
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
                ON CONFLICT (name) DO UPDATE SET num_tagged = tags.num_tagged + 1
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
    /// Whether or not the thread is hidden
    pub hidden:    bool,
}

impl Reply {
    pub async fn fetch(conn: &PgPool, id: i32) -> Result<Self, sqlx::Error> {
        sqlx::query_as("SELECT * FROM replies WHERE id = $1")
            .bind(id)
            .fetch_one(conn)
            .await
    }

    pub async fn fetch_optional(conn: &PgPool, id: i32) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM replies WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await
    }
}

#[derive(Serialize, Error, Debug)]
pub enum DeleteReplyError {
    #[error("You are not privileged enough")]
    Unprivileged,
    #[error("No such reply exists")]
    NoSuchReply,
    #[error("You cannot delete the first reply in a thread")]
    CannotDeleteFirstReply,
    #[error("Internal database error: {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
}

post!(
    "/delete_reply/:dead_reply_id",
    #[json_result]
    async fn delete_reply(
        conn: Extension<PgPool>,
        user: User,
        Path(dead_reply_id): Path<i32>,
    ) -> Json<Result<(), DeleteReplyError>> {
        if user.role < Role::Moderator {
            return Err(DeleteReplyError::Unprivileged);
        }

        let dead_reply = Reply::fetch_optional(&*conn, dead_reply_id)
            .await?
            .ok_or(DeleteReplyError::NoSuchReply)?;

        // Get the post before this one in case last_post is the dead reply
        let prev_reply: Reply = sqlx::query_as(
            "SELECT * FROM replies WHERE thread_id = $1 AND id < $2 ORDER BY id DESC",
        )
        .bind(dead_reply.thread_id)
        .bind(dead_reply_id)
        .fetch_optional(&*conn)
        .await?
        .ok_or(DeleteReplyError::CannotDeleteFirstReply)?;

        sqlx::query("UPDATE threads SET last_post = $1 WHERE id = $2 AND last_post = $3")
            .bind(prev_reply.id)
            .bind(dead_reply.thread_id)
            .bind(dead_reply_id)
            .execute(&*conn)
            .await?;

        // Reduce the number of replies by one:
        sqlx::query("UPDATE threads SET num_replies = num_replies - 1 WHERE id = $1")
            .bind(dead_reply.thread_id)
            .execute(&*conn)
            .await?;

        // Delete the reply:
        sqlx::query("DELETE FROM replies WHERE id = $1")
            .bind(dead_reply_id)
            .execute(&*conn)
            .await?;

        tracing::info!(
            "User `{}` has deleted reply {dead_reply_id} in thread {}",
            user.name,
            dead_reply.thread_id,
        );

        Ok(())
    }
);

#[derive(Deserialize)]
pub struct ReplyForm {
    body:      String,
    thread_id: String,
}

#[derive(Debug, Serialize, Error)]
pub enum ReplyError {
    #[error("no such thread")]
    NoSuchThread,
    #[error("reply cannot be empty")]
    ReplyIsEmpty,
    #[error("thread is locked")]
    ThreadIsLocked,
    #[error("error uploading image: {0}")]
    UploadImageError(
        #[from]
        #[serde(skip)]
        UploadImageError,
    ),
    #[error("internal db error: {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
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
        if Thread::fetch_optional(&conn, thread_id)
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
                    ($1, $2, $3, $4, $5, $6, $7, $8, '{}')
                RETURNING *
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

        let thread: Thread = sqlx::query_as(
            r#"
            UPDATE threads SET
                last_post = $1,
                num_replies = num_replies + 1
            RETURNING *
            "#)
            .bind(reply.id)
            .fetch_one(&mut *transaction)
            .await?;

        transaction.commit().await?;

        user.read_thread(&*conn, &thread).await?;

        Ok(reply)
    }
}

#[derive(Deserialize)]
pub struct UpdateReplyParams {
    hidden: Option<bool>,
}

#[derive(Deserialize)]
pub struct UpdateReplyForm {
    body: Option<String>,
}

#[derive(Serialize, Error, Debug)]
pub enum UpdateReplyError {
    #[error("You are not privileged enough")]
    Unprivileged,
    #[error("Post does not exist")]
    NoSuchReply,
    #[error("This is not your post")]
    NotYourReply,
    #[error("You cannot make a post empty")]
    CannotMakeEmpty,
    #[error("Internal database error: {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
}

post! {
    "/reply/:post_id",
    #[json_result]
    pub async fn update_reply(
        conn: Extension<PgPool>,
        user: User,
        Path(post_id): Path<i32>,
        Query(UpdateReplyParams {
            hidden,
        }): Query<UpdateReplyParams>,
        Form(UpdateReplyForm { body }): Form<UpdateReplyForm>,
    ) -> Json<Result<(), UpdateReplyError>> {
        let post = Reply::fetch_optional(&*conn, post_id)
            .await?
            .ok_or(UpdateReplyError::NoSuchReply)?;

        if let Some(hidden) = hidden {
            if user.role < Role::Moderator {
                return Err(UpdateReplyError::Unprivileged);
            }
            sqlx::query("UPDATE replies SET hidden = $1 WHERE id = $2")
                .bind(hidden)
                .bind(post_id)
                .execute(&*conn)
                .await?;
        }

        let Some(body) = body else {
            return Ok(());
        };

        if post.author_id != user.id && user.role < Role::Moderator {
            return Err(UpdateReplyError::NotYourReply);
        }

        let body = body.trim();

        if post.image.is_none() && body.is_empty() {
            return Err(UpdateReplyError::CannotMakeEmpty);
        }

        sqlx::query("UPDATE replies SET body = $1 WHERE id = $2")
            .bind(body)
            .bind(post_id)
            .execute(&*conn)
            .await?;

        Ok(())
    }
}

#[derive(Serialize, Error, Debug)]
pub enum ReactError {
    #[error("No such reply exists")]
    NoSuchReply,
    #[error("You don't own these reactions")]
    NotYourItems,
    #[error("You have already consumed one of these reactions")]
    AlreadyConsumed,
    #[error("You cannot react to your own post")]
    ThisIsYourPost,
    #[error("Internal database error: {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
}

post!(
    "/react/:post_id",
    #[json_result]
    pub async fn react(
        conn: Extension<PgPool>,
        user: User,
        Path(post_id): Path<i32>,
        Form(used_reactions): Form<HashMap<i32, String>>,
    ) -> Json<Result<(), ReactError>> {
        let reply = Reply::fetch_optional(&conn, post_id)
            .await?
            .ok_or(ReactError::NoSuchReply)?;

        if reply.author_id == user.id {
            return Err(ReactError::ThisIsYourPost);
        }

        let mut transaction = conn.begin().await?;
        let mut new_reactions = Vec::new();
        let author = User::fetch(&mut transaction, reply.author_id).await?;

        // Verify that all of the reactions are owned by the user:
        for (reaction, selected) in used_reactions.into_iter() {
            let item_drop = ItemDrop::fetch(&mut transaction, reaction).await?;
            let item = item_drop.fetch_item(&mut transaction).await?;
            if selected != "on" || item_drop.owner_id != user.id || !item.is_reaction() {
                return Err(ReactError::NotYourItems);
            }

            // Set the drops to consumed:
            if sqlx::query("UPDATE drops SET consumed = TRUE WHERE id = $1 AND consumed = FALSE")
                .bind(reaction)
                .execute(&mut transaction)
                .await?
                .rows_affected()
                != 1
            {
                return Err(ReactError::AlreadyConsumed);
            }

            new_reactions.push(reaction);
            author
                .add_experience(&mut transaction, item.get_experience().unwrap() as i64)
                .await?;
        }

        sqlx::query("UPDATE replies SET reactions = reactions || $1 WHERE id = $2")
            .bind(new_reactions)
            .bind(post_id)
            .execute(&mut transaction)
            .await?;

        transaction.commit().await?;

        Ok(())
    }
);

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

/*
pub struct ImageUploadResult {

}
*/

#[derive(Debug, Serialize, Error)]
pub enum UploadImageError {
    #[error("invalid file type")]
    InvalidExtension,
    #[error("error decoding image: {0}")]
    ImageError(
        #[from]
        #[serde(skip)]
        image::ImageError,
    ),
    #[error("internal server error: {0}")]
    InternalServerError(
        #[from]
        #[serde(skip)]
        tokio::task::JoinError,
    ),
    #[error("internal block storage error: {0}")]
    InternalBlockStorageError(
        #[from]
        #[serde(skip)]
        SdkError<PutObjectError>,
    ),
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
