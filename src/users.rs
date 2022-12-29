use std::{
    collections::HashMap,
    ops::Range,
    string::FromUtf8Error,
    sync::{Arc, Mutex},
};

use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit, Nonce};
use askama::Template;
use axum::{
    async_trait,
    extract::{Extension, Form, FromRequestParts, Path, Query},
    http::request::Parts,
    response::{IntoResponse, Redirect, Response},
};
use axum_client_ip::ClientIp;
use chrono::{prelude::*, Duration};
use cookie::time as cookie_time;
use futures::StreamExt;
use google_authenticator::{create_secret, qr_code_url};
use ipnetwork::IpNetwork;
use lazy_static::lazy_static;
use libpasta::{hash_password, verify_password};
use marche_proc_macros::{json, ErrorCode};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgExecutor, PgPool, Postgres, Row, Transaction, Type};
use thiserror::Error;
use tower_cookies::{Cookie, Cookies, Key};

use crate::{
    items::{Item, ItemDrop},
    post,
    threads::{Reply, Thread},
};

#[derive(FromRow, Debug)]
pub struct User {
    /// Id of the user
    pub id:                    i32,
    /// User name (lowercased copy of the display name for faster lookups)
    pub name:                  String,
    /// Display name
    pub display_name:          String,
    /// Password hash
    pub password:              String,
    /// Encrypted, shared secret for 2FA
    pub secret:                Vec<u8>,
    /// Reset code to change password and 2FA
    pub reset_code:            String,
    /// Biography of the user
    pub bio:                   String,
    /// Email address of the useer
    pub email:                 String,
    /// Role
    pub role:                  Role,
    /// Exprerience
    pub experience:            i64,
    /// Last reward
    pub last_reward:           NaiveDateTime,
    /// ProfilePic equipment slot
    pub equip_slot_prof_pic:   Option<i32>,
    /// ProfileBackground equipment slot
    pub equip_slot_background: Option<i32>,
    /// Badge equipment slots
    pub equip_slot_badges:     Vec<i32>,
    /// If the user is banned, and for how long
    pub banned_until:          Option<NaiveDateTime>,
    /// Notes on the user by moderators or admins
    pub notes:                 String,
}

/// Displayable user profile
#[derive(Clone, Serialize)]
pub struct ProfileStub {
    pub id:         i32,
    pub name:       String,
    pub picture:    Option<String>,
    pub background: Option<String>,
    pub badges:     Vec<String>,
    pub level:      LevelInfo,
}

#[derive(
    Copy, Clone, Debug, Hash, Eq, PartialEq, PartialOrd, Ord, Serialize, Deserialize, Type,
)]
#[sqlx(type_name = "user_role")]
#[sqlx(rename_all = "snake_case")]
pub enum Role {
    User,
    Moderator,
    Admin,
}

#[derive(Copy, Clone, Serialize)]
pub struct LevelInfo {
    pub level:         u32,
    pub curr_xp:       u64,
    pub next_level_xp: u64,
}

pub const MAX_NUM_BADGES: usize = 10;

impl User {
    pub async fn fetch(conn: impl PgExecutor<'_>, user_id: i32) -> Result<Self, sqlx::Error> {
        sqlx::query_as("SELECT * FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(conn)
            .await
    }

    pub async fn fetch_optional(
        conn: impl PgExecutor<'_>,
        user_id: i32,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(conn)
            .await
    }

    /// Returns the raw, total experience of the user
    pub fn experience(&self) -> u64 {
        self.experience as u64
    }

    /// Returns the level of the user. The level is defined as the log_2 of the
    /// user's experience value.
    pub fn level(&self) -> u32 {
        let xp = self.experience();
        // Base level is 1
        if xp < 4 {
            1
        } else {
            63 - xp.leading_zeros()
        }
    }

    pub fn is_banned(&self) -> bool {
        self.banned_until
            .map(|until| {
                let now = Utc::now().naive_utc();
                now < until
            })
            .unwrap_or(false)
    }

    /// Returns a range of the current completion of the user's next level.
    pub fn level_completion(&self) -> Range<u64> {
        let level = self.level();
        let base_xp = if level == 1 { 0 } else { 1 << level };
        let next_level = level + 1;
        let next_level_xp = (1 << next_level) as u64 - base_xp;
        (self.experience() - base_xp)..next_level_xp
    }

    pub fn level_info(&self) -> LevelInfo {
        let completion = self.level_completion();
        LevelInfo {
            level:         self.level(),
            curr_xp:       completion.start,
            next_level_xp: completion.end,
        }
    }

    pub async fn add_experience(
        &self,
        conn: &mut Transaction<'_, Postgres>,
        xp: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE users SET experience = GREATEST(experience + $1, 0) WHERE id = $2")
            .bind(xp)
            .bind(self.id)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Returns a vec of equipped items.
    pub async fn equipped(&self, conn: &PgPool) -> Result<Vec<(Item, ItemDrop)>, sqlx::Error> {
        let mut items = Vec::new();

        if let Some(prof_pic) = self.equip_slot_prof_pic {
            let item_drop = ItemDrop::fetch(conn, prof_pic).await?;
            items.push((item_drop.fetch_item(conn).await?, item_drop));
        }

        if let Some(background) = self.equip_slot_background {
            let item_drop = ItemDrop::fetch(conn, background).await?;
            items.push((item_drop.fetch_item(conn).await?, item_drop));
        }

        for badge in self.equip_slot_badges.iter() {
            let item_drop = ItemDrop::fetch(conn, *badge).await?;
            items.push((item_drop.fetch_item(conn).await?, item_drop));
        }

        Ok(items)
    }

    pub async fn inventory(
        &self,
        conn: &PgPool,
    ) -> Result<impl Iterator<Item = (Item, ItemDrop)>, sqlx::Error> {
        let mut inventory =
            sqlx::query_as("SELECT * FROM drops WHERE owner_id = $1 AND consumed = FALSE")
                .bind(self.id)
                .fetch(conn)
                .filter_map(|item_drop: Result<ItemDrop, _>| async move {
                    let Ok(item_drop) = item_drop else {
                        return None;
                    };
                    Item::fetch(conn, item_drop.item_id)
                        .await
                        .ok()
                        .map(move |item| (item, item_drop))
                })
                .collect::<Vec<_>>()
                .await;

        inventory.sort_by(|a, b| a.0.rarity.cmp(&b.0.rarity).reverse());

        Ok(inventory.into_iter())
    }

    pub async fn get_avatar(&self, conn: &PgPool) -> Result<Option<String>, sqlx::Error> {
        let Some(drop_id) = self.equip_slot_prof_pic else {
            return Ok(None);
        };
        Ok(ItemDrop::fetch(conn, drop_id)
            .await?
            .fetch_item(conn)
            .await?
            .as_avatar())
    }

    pub async fn get_profile_background(
        &self,
        conn: &PgPool,
    ) -> Result<Option<String>, sqlx::Error> {
        let Some(drop_id) = self.equip_slot_background else {
            return Ok(None);
        };
        let item_drop = ItemDrop::fetch(conn, drop_id).await?;
        Ok(item_drop
            .fetch_item(conn)
            .await?
            .as_profile_background(item_drop.pattern))
    }

    pub async fn get_badges(&self, conn: &PgPool) -> Result<Vec<String>, sqlx::Error> {
        let mut badges = Vec::new();
        for badge in self.equip_slot_badges.iter() {
            let Some(item) = ItemDrop::fetch(conn, *badge).await?.fetch_item(&*conn).await?.as_badge() else {
                continue;
            };
            badges.push(item);
        }
        Ok(badges)
    }

    /// Attempt to update the last drop time. If we fail, return false.
    /// This will fail if the user has received a new reward since the user has
    /// been fetched, which is by design.
    pub async fn update_last_reward(&self, conn: impl PgExecutor<'_>) -> Result<bool, sqlx::Error> {
        let rows_affected =
            sqlx::query("UPDATE users SET last_reward = $1 WHERE id = $2 AND last_reward = $3")
                .bind(Utc::now().naive_utc())
                .bind(self.id)
                .bind(self.last_reward)
                .execute(conn)
                .await?
                .rows_affected();
        Ok(rows_affected > 0)
    }

    pub async fn get_profile_stub(&self, conn: &PgPool) -> Result<ProfileStub, sqlx::Error> {
        Ok(ProfileStub {
            id:         self.id,
            name:       self.display_name.clone(),
            picture:    self.get_avatar(conn).await?,
            background: self.get_profile_background(conn).await?,
            badges:     self.get_badges(conn).await?,
            level:      self.level_info(),
        })
    }

    pub async fn next_unread(&self, conn: &PgPool, thread: &Thread) -> Result<i32, sqlx::Error> {
        let reading_history: Option<ReadingHistory> =
            sqlx::query_as("SELECT * FROM reading_history WHERE reader_id = $1 AND thread_id = $2")
                .bind(self.id)
                .bind(thread.id)
                .fetch_optional(conn)
                .await?;

        let last_read = match reading_history {
            None => {
                // Find the first reply
                let reply: Reply = sqlx::query_as(
                    "SELECT * FROM replies WHERE thread_id = $1 ORDER BY post_date ASC",
                )
                .bind(thread.id)
                .fetch_one(conn)
                .await?;

                reply.id
            }
            Some(ReadingHistory { last_read, .. }) => sqlx::query_as(
                "SELECT * FROM replies WHERE thread_id = $1 AND id > $2 ORDER BY post_date ASC",
            )
            .bind(thread.id)
            .bind(last_read)
            .fetch_optional(conn)
            .await?
            .map_or_else(|| last_read, |reply: Reply| reply.id),
        };

        Ok(last_read)
    }

    pub async fn has_read(&self, conn: &PgPool, thread: &Thread) -> Result<bool, sqlx::Error> {
        Ok(
            sqlx::query_as("SELECT * FROM reading_history WHERE reader_id = $1 AND thread_id = $2")
                .bind(self.id)
                .bind(thread.id)
                .fetch_optional(conn)
                .await?
                .map_or(false, |history: ReadingHistory| {
                    history.last_read >= thread.last_post
                }),
        )
    }

    pub async fn read_thread(&self, conn: &PgPool, thread: &Thread) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO reading_history
                (reader_id, thread_id, last_read)
            VALUES
                ($1, $2, $3)
            ON CONFLICT
                (reader_id, thread_id)
            DO UPDATE SET
                last_read = EXCLUDED.last_read
            "#,
        )
        .bind(self.id)
        .bind(thread.id)
        .bind(thread.last_post)
        .execute(conn)
        .await?;

        Ok(())
    }

    pub async fn incoming_offers(&self, conn: &PgPool) -> Result<i64, sqlx::Error> {
        Ok(
            sqlx::query("SELECT COUNT(*) FROM trade_requests WHERE receiver_id = $1")
                .bind(self.id)
                .fetch_one(conn)
                .await?
                .get(0),
        )
    }
}

#[derive(Deserialize)]
pub struct UserRegistrationForm {
    username: String,
    password: String,
    email:    String,
}

#[derive(Serialize)]
pub struct UserRegistration {
    qr_code_url: String,
    reset_code:  String,
}

#[derive(Error, Debug, Serialize, ErrorCode)]
pub enum UserRegistrationError {
    #[error("User names can only contain alphabetical characters")]
    InvalidUserName,
    #[error("Password is too short (minimum {MINIMUM_PASSWORD_LENGTH} characters)")]
    PasswordTooShort,
    #[error("User name has already been registered")]
    UserNameInUse,
    #[error("Invalid email")]
    InvalidEmail,
    #[error("Internal db error: {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
    #[error("Internal encryption error")]
    InternalEncryptionError,
}

impl From<aes_gcm::Error> for UserRegistrationError {
    fn from(_: aes_gcm::Error) -> Self {
        UserRegistrationError::InternalEncryptionError
    }
}

const MINIMUM_PASSWORD_LENGTH: usize = 8;

post!(
    "/user",
    #[json]
    async fn register_user(
        conn: Extension<PgPool>,
        Form(UserRegistrationForm {
            username,
            password,
            email,
        }): Form<UserRegistrationForm>,
    ) -> Result<UserRegistration, UserRegistrationError> {
        let username = username.trim();
        if !is_valid_username(&username) {
            return Err(UserRegistrationError::InvalidUserName);
        }

        let name = username.to_lowercase();
        let display_name = username;
        let email = email.trim();

        if password.len() < MINIMUM_PASSWORD_LENGTH {
            return Err(UserRegistrationError::PasswordTooShort);
        }

        if email.is_empty() {
            return Err(UserRegistrationError::InvalidEmail);
        }

        let existing_user: Option<User> = sqlx::query_as("SELECT * FROM users WHERE name = $1")
            .bind(&name)
            .fetch_optional(&*conn)
            .await?;

        if existing_user.is_some() {
            return Err(UserRegistrationError::UserNameInUse);
        }

        let shared_secret = create_secret!();
        let nonce = Nonce::from_slice(SHARED_SECRET_NONCE);
        let encrypted_secret = SHARED_SECRET_CIPHER.encrypt(nonce, shared_secret.as_ref())?;
        let qr_code_url = qr_code_url!(&shared_secret, "C'est Le Marché", "C'est Le Marché");

        let reset_code =
            base64::encode_config(&rand::random::<[u8; 32]>(), base64::URL_SAFE_NO_PAD);
        let hashed_reset_code = hash_password(&reset_code);
        let password = hash_password(&password);

        sqlx::query(
            r#"
            INSERT INTO users (
                name, display_name, password, secret, reset_code, email,
                role, last_reward, experience, bio, equip_slot_badges, notes
            ) VALUES ( $1, $2, $3, $4, $5, $6, $7, $8, 0, '', '{}', '' )
            "#,
        )
        .bind(name)
        .bind(display_name)
        .bind(password)
        .bind(encrypted_secret)
        .bind(&hashed_reset_code)
        .bind(email.trim())
        .bind(Role::User)
        .bind(Utc::now().naive_utc())
        .execute(&*conn)
        .await?;

        Ok(UserRegistration {
            qr_code_url,
            reset_code,
        })
    }
);

fn is_valid_username(username: &str) -> bool {
    username.chars().all(char::is_alphanumeric)
}

#[derive(Deserialize)]
pub struct UpdateUser {
    role: Role,
}

#[derive(Debug, Error, Serialize, ErrorCode)]
pub enum UpdateUserError {
    #[error("You are not privileged enough")]
    Unauthorized,
    #[error("There is no such user")]
    NoSuchUser,
    #[error("Internal database error: {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
}

post!(
    "/user/:user_id",
    #[json]
    async fn update_user(
        conn: Extension<PgPool>,
        moderator: User,
        Path(user_id): Path<i32>,
        Query(UpdateUser { role }): Query<UpdateUser>,
    ) -> Result<(), UpdateUserError> {
        let user = User::fetch_optional(&*conn, user_id)
            .await?
            .ok_or(UpdateUserError::NoSuchUser)?;

        if user.role >= moderator.role || role >= moderator.role {
            return Err(UpdateUserError::Unauthorized);
        }

        sqlx::query("UPDATE users SET role = $1 WHERE id = $2")
            .bind(role)
            .bind(user_id)
            .execute(&*conn)
            .await?;

        Ok(())
    }
);

#[derive(Deserialize)]
pub struct BanUser {
    #[serde(default, deserialize_with = "crate::empty_string_as_none")]
    ban_len: Option<u32>,
}

post!(
    "/ban/:user_id",
    #[json]
    async fn ban_user(
        conn: Extension<PgPool>,
        moderator: User,
        Path(user_id): Path<i32>,
        Query(BanUser { ban_len }): Query<BanUser>,
    ) -> Result<(), UpdateUserError> {
        if moderator.role < Role::Moderator || moderator.id == user_id {
            return Err(UpdateUserError::Unauthorized);
        }
        User::fetch_optional(&*conn, user_id)
            .await?
            .ok_or(UpdateUserError::NoSuchUser)?;

        sqlx::query("UPDATE users SET banned_until = $1 WHERE id = $2")
            .bind(ban_len.map(|days| (Utc::now() + Duration::days(days as i64)).naive_utc()))
            .bind(user_id)
            .execute(&*conn)
            .await?;

        Ok(())
    }
);

#[derive(Deserialize)]
pub struct UpdateBioForm {
    bio: String,
}

#[derive(Debug, Serialize, Error, ErrorCode)]
pub enum UpdateBioError {
    #[error("Bio is too long (maximum {MAX_BIO_LEN} characters allowed)")]
    TooLong,
    #[error("Internal database error: {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
}

pub const MAX_BIO_LEN: usize = 300;

post!(
    "/bio",
    #[json]
    async fn update_bio(
        conn: Extension<PgPool>,
        user: User,
        Form(UpdateBioForm { bio }): Form<UpdateBioForm>,
    ) -> Result<(), UpdateBioError> {
        if bio.len() > MAX_BIO_LEN {
            return Err(UpdateBioError::TooLong);
        }

        sqlx::query("UPDATE users SET bio = $1 WHERE id = $2")
            .bind(bio)
            .bind(user.id)
            .execute(&*conn)
            .await?;

        Ok(())
    }
);

#[derive(Deserialize)]
pub struct AddNoteForm {
    body: String,
}

#[derive(Debug, Error, Serialize, ErrorCode)]
pub enum AddNoteError {
    #[error("You are not privileged enough")]
    Unauthorized,
    #[error("Internal database error: {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
}

post!(
    "/add_note/:user_id",
    #[json]
    pub async fn submit(
        conn: Extension<PgPool>,
        viewer: User,
        Path(user_id): Path<i32>,
        Form(AddNoteForm { body }): Form<AddNoteForm>,
    ) -> Result<(), AddNoteError> {
        if viewer.role < Role::Moderator {
            return Err(AddNoteError::Unauthorized);
        }

        let viewer_name = viewer.name;
        let body = html_escape::encode_text(&body);
        let new_note = format!("<p>“{body}” — {viewer_name}</p>");

        sqlx::query("UPDATE users SET notes = notes || $1 WHERE id = $2")
            .bind(new_note)
            .bind(user_id)
            .execute(&*conn)
            .await?;

        Ok(())
    }
);

/// Name of the cookie we use to store the session Id.
const USER_SESSION_ID_COOKIE: &str = "session_id";
// TODO: Move to environmental variable
const PRIVATE_COOKIE_KEY: &str = "ea63npVp7Vg+ileGuoO0OJbBLOdSkHKkNwu87B8/joU=";

#[async_trait]
impl<S> FromRequestParts<S> for User
where
    S: Send + Sync,
{
    type Rejection = UserRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let redirect = parts
            .uri
            .path_and_query()
            .map(|x| x.as_str().to_string())
            .unwrap_or_else(String::new);
        let cookies = Cookies::from_request_parts(parts, state)
            .await
            .map_err(|_| UserRejection::Unauthorized {
                redirect: redirect.clone(),
            })?;
        let private_key = Key::derive_from(PRIVATE_COOKIE_KEY.as_bytes());
        let signed = cookies.private(&private_key);
        let session_id = signed
            .get(USER_SESSION_ID_COOKIE)
            .ok_or(UserRejection::Unauthorized {
                redirect: redirect.clone(),
            })?;
        let conn = Extension::<PgPool>::from_request_parts(parts, state)
            .await
            .map_err(|_| UserRejection::UnknownError)?;
        let Some(session) = LoginSession::fetch(&conn, session_id.value()).await? else {
            return Err(UserRejection::Unauthorized {
                redirect: redirect.clone(),
            });
        };
        let user = match User::fetch_optional(&*conn, session.user_id).await {
            Ok(Some(user)) => user,
            Ok(None) => return Err(UserRejection::UnknownUser),
            Err(_) => return Err(UserRejection::Unauthorized { redirect }),
        };
        if user.is_banned() {
            Err(UserRejection::Banned {
                until: user.banned_until.unwrap(),
            })
        } else {
            Ok(user)
        }
    }
}

#[derive(Debug, Error)]
pub enum UserRejection {
    #[error("An unknown error occurred")]
    UnknownError,
    #[error("Internal database error: {0}")]
    InternalDbError(#[from] sqlx::Error),
    #[error("Unknown user")]
    UnknownUser,
    #[error("Unauthorized user")]
    Unauthorized { redirect: String },
    #[error("Banned until {until}")]
    Banned { until: NaiveDateTime },
}

#[derive(Template)]
#[template(path = "banned.html")]
pub struct Banned {
    judge_type: bool,
    until:      String,
}

impl IntoResponse for UserRejection {
    fn into_response(self) -> Response {
        match self {
            Self::Banned { until } => Banned {
                judge_type: rand::random(),
                until:      until.format(crate::DATE_FMT).to_string(),
            }
            .into_response(),
            Self::Unauthorized { redirect } => {
                Redirect::to(&format!("/login?redirect={redirect}")).into_response()
            }
            err => {
                tracing::error!("Unknown error occurred: {:?}", err);
                Redirect::to("/login").into_response()
            }
        }
    }
}

#[derive(Clone)]
pub struct UserCache<'a> {
    conn:   &'a PgPool,
    cached: Arc<Mutex<HashMap<i32, Arc<ProfileStub>>>>,
}

impl<'a> UserCache<'a> {
    pub fn new(conn: &'a PgPool) -> Self {
        UserCache {
            conn,
            cached: Arc::new(Mutex::new(Default::default())),
        }
    }

    pub async fn get(&self, id: i32) -> Result<Arc<ProfileStub>, sqlx::Error> {
        if let Some(result) = self.cached.lock().unwrap().get(&id) {
            return Ok(result.clone());
        }
        let profile_stub = Arc::new(
            User::fetch(self.conn, id)
                .await?
                .get_profile_stub(self.conn)
                .await?,
        );
        self.cached.lock().unwrap().insert(id, profile_stub.clone());
        Ok(profile_stub)
    }
}

/// User login sessions
#[derive(FromRow)]
pub struct LoginSession {
    /// Id of the login session
    pub id:            i32,
    /// Auth token
    pub session_id:    String,
    /// UserId of the session
    pub user_id:       i32,
    /// When the session began
    pub session_start: NaiveDateTime,
    /// The IP address of the connecting client
    pub ip_addr:       IpNetwork,
}

#[derive(Debug, Serialize, Error, ErrorCode)]
pub enum LoginFailure {
    #[error("Username or password is incorrect")]
    UserOrPasswordIncorrect,
    #[error("Internal database error: {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
    #[error("Internal encryption error")]
    InternalEncryptionError,
}

impl From<aes_gcm::Error> for LoginFailure {
    fn from(_: aes_gcm::Error) -> Self {
        LoginFailure::InternalEncryptionError
    }
}

impl From<FromUtf8Error> for LoginFailure {
    fn from(_: FromUtf8Error) -> Self {
        LoginFailure::InternalEncryptionError
    }
}

lazy_static! {
    pub static ref SHARED_SECRET_CIPHER: Aes256Gcm = {
        let key = std::env::var("SHARED_SECRET_KEY").unwrap();
        let key_bytes = base64::decode(&key).expect("SHARED_SECRET_KEY is not valid base64");
        Aes256Gcm::new_from_slice(&key_bytes).expect("could not construct shared secret cipher")
    };
}

pub const SHARED_SECRET_NONCE: &[u8; 12] = b"96bitsIs12u8";

impl LoginSession {
    /// Fetch the login session.
    pub async fn fetch(conn: &PgPool, session_id: &str) -> Result<Option<Self>, sqlx::Error> {
        Ok(
            sqlx::query_as("SELECT * FROM login_sessions WHERE session_id = $1")
                .bind(session_id)
                .fetch_optional(conn)
                .await?
                .filter(|session: &Self| {
                    // The session is automatically invalid if the session is longer than a month
                    // old.
                    session.session_start >= (Utc::now() - Duration::weeks(52)).naive_utc()
                }),
        )
    }

    /// Attempt to login a user
    pub async fn login(
        conn: &PgPool,
        username: &str,
        password: &str,
        ip_addr: IpNetwork,
    ) -> Result<Self, LoginFailure> {
        let username = username.trim();
        let user: User = {
            sqlx::query_as("SELECT * FROM users WHERE name = $1")
                .bind(username.to_lowercase())
                .fetch_optional(conn)
                .await?
                .ok_or(LoginFailure::UserOrPasswordIncorrect)?
        };

        if !verify_password(&user.password, password) {
            return Err(LoginFailure::UserOrPasswordIncorrect);
        }

        // TODO: Add extra protections here?

        let mut key = [0u8; 16];
        OsRng.fill_bytes(&mut key);

        // Found a user, create a new login sessions
        let session_start = Utc::now().naive_utc();

        Ok(sqlx::query_as(
            r#"
                INSERT INTO login_sessions
                    (user_id, session_id, session_start, ip_addr)
                VALUES
                    ($1, $2, $3, $4)
                RETURNING
                    *
            "#,
        )
        .bind(user.id)
        .bind(i128::from_be_bytes(key).to_string())
        .bind(session_start)
        .bind(ip_addr)
        .fetch_one(conn)
        .await?)
    }
}

#[derive(Deserialize)]
pub struct LoginForm {
    username: String,
    password: String,
}

post!(
    "/login",
    #[json]
    async fn login(
        pool: Extension<PgPool>,
        jar: Cookies,
        ClientIp(ip): ClientIp,
        login: Form<LoginForm>,
    ) -> Result<(), LoginFailure> {
        let key = Key::derive_from(PRIVATE_COOKIE_KEY.as_bytes());
        let private = jar.private(&key);
        private.remove(Cookie::named(USER_SESSION_ID_COOKIE));
        let LoginSession { session_id, .. } = LoginSession::login(
            &pool,
            login.username.trim(),
            login.password.trim(),
            IpNetwork::from(ip),
        )
        .await?;
        let mut cookie = Cookie::new(USER_SESSION_ID_COOKIE, session_id.to_string());
        cookie
            .set_expires(cookie_time::OffsetDateTime::now_utc() + cookie_time::Duration::weeks(52));
        private.add(cookie);
        Ok(())
    }
);

#[derive(Debug, Serialize, Error, ErrorCode)]
pub enum LogoutFailure {
    #[error("An unknown error occurred")]
    UnknownError,
    #[error("Internal database error: {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
}

post! {
    "/logout",
    #[json]
    async fn logout(
        pool: Extension<PgPool>,
        cookies: Cookies,
    ) -> Result<(), LogoutFailure> {
        let private_key = Key::derive_from(PRIVATE_COOKIE_KEY.as_bytes());
        let signed = cookies.private(&private_key);
        let session_id = signed
            .get(USER_SESSION_ID_COOKIE)
            .ok_or(LogoutFailure::UnknownError)?;

        sqlx::query("DELETE FROM login_sessions WHERE id = $1")
            .bind(session_id.value())
            .execute(&*pool)
            .await?;

        Ok(())
    }
}

#[derive(FromRow)]
pub struct ReadingHistory {
    pub id:        i32,
    pub reader_id: i32,
    pub thread_id: i32,
    pub last_read: i32,
}
