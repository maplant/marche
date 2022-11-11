use std::{
    collections::HashMap,
    ops::Range,
    sync::{Arc, Mutex},
};

use askama::Template;
use axum::{
    async_trait,
    extract::{Extension, Form, FromRequest, Path, Query, RequestParts},
    response::{IntoResponse, Redirect, Response},
};
use axum_client_ip::ClientIp;
use chrono::{prelude::*, Duration};
use futures::StreamExt;
use ipnetwork::IpNetwork;
use libpasta::verify_password;
use marche_proc_macros::json_result;
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
    /// User name
    pub name:                  String,
    /// Password hash
    pub password:              String,
    /// Biography of the user
    pub bio:                   String,
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
            name:       self.name.clone(),
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
pub struct UpdateUser {
    role: Role,
}

#[derive(Debug, Error, Serialize)]
pub enum UpdateUserError {
    #[error("You are not privileged enough")]
    Unprivileged,
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
    #[json_result]
    async fn update_user(
        conn: Extension<PgPool>,
        moderator: User,
        Path(user_id): Path<i32>,
        Query(UpdateUser { role }): Query<UpdateUser>,
    ) -> Json<Result<(), UpdateUserError>> {
        let user = User::fetch_optional(&*conn, user_id)
            .await?
            .ok_or(UpdateUserError::NoSuchUser)?;

        if user.role >= moderator.role || role >= moderator.role {
            return Err(UpdateUserError::Unprivileged);
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
    #[json_result]
    async fn ban_user(
        conn: Extension<PgPool>,
        moderator: User,
        Path(user_id): Path<i32>,
        Query(BanUser { ban_len }): Query<BanUser>,
    ) -> Json<Result<(), UpdateUserError>> {
        if moderator.role < Role::Moderator || moderator.id == user_id {
            return Err(UpdateUserError::Unprivileged);
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

#[derive(Debug, Serialize, Error)]
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
    #[json_result]
    async fn update_bio(
        conn: Extension<PgPool>,
        user: User,
        Form(UpdateBioForm { bio }): Form<UpdateBioForm>,
    ) -> Json<Result<(), UpdateBioError>> {
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

#[derive(Debug, Error, Serialize)]
pub enum AddNoteError {
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
    "/add_note/:user_id",
    #[json_result]
    pub async fn submit(
        conn: Extension<PgPool>,
        viewer: User,
        Path(user_id): Path<i32>,
        Form(AddNoteForm { body }): Form<AddNoteForm>,
    ) -> Json<Result<(), AddNoteError>> {
        if viewer.role < Role::Moderator {
            return Err(AddNoteError::Unprivileged);
        }

        let viewer_name = viewer.name;
        let body = html_escape::encode_text(&body);
        let new_note = format!("<p>“{body}” — {viewer_name}</p>");

        sqlx::query("UPDATE users SET notes = notes || $1 WHERE id = $1")
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
impl<B> FromRequest<B> for User
where
    B: Send,
{
    type Rejection = UserRejection;

    async fn from_request(req: &mut RequestParts<B>) -> Result<Self, Self::Rejection> {
        let redirect = req
            .uri()
            .path_and_query()
            .map(|x| x.as_str().to_string())
            .unwrap_or_else(String::new);
        let cookies =
            Cookies::from_request(req)
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
        let conn = Extension::<PgPool>::from_request(req)
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
            _ => todo!(),
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

#[derive(Debug, Serialize, Error)]
pub enum LoginFailure {
    #[error("username or password is incorrect")]
    UserOrPasswordIncorrect,
    #[error("internal database error: {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
}

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
                    session.session_start >= (Utc::now() - Duration::weeks(4)).naive_utc()
                }),
        )
    }

    /// Attempt to login a user
    pub async fn login(
        conn: &PgPool,
        user_name: &str,
        password: &str,
        ip_addr: IpNetwork,
    ) -> Result<Self, LoginFailure> {
        let user_name = user_name.trim();
        let user: User = {
            sqlx::query_as("SELECT * FROM users WHERE name = $1")
                .bind(user_name)
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

post! {
    "/login",
    #[json_result]
    async fn login(
        pool: Extension<PgPool>,
        jar: Cookies,
        ClientIp(ip): ClientIp,
        login: Form<LoginForm>,
    ) -> Json<Result<(), LoginFailure>> {
        let key = Key::derive_from(PRIVATE_COOKIE_KEY.as_bytes());
        let private = jar.private(&key);
        private.remove(Cookie::named(USER_SESSION_ID_COOKIE));
        let LoginSession { session_id, .. } =
            LoginSession::login(&pool, &login.username, &login.password, IpNetwork::from(ip)).await?;
        private.add(Cookie::new(USER_SESSION_ID_COOKIE, session_id.to_string()));
        Ok(())
    }
}

#[derive(Debug, Serialize, Error)]
pub enum LogoutFailure {
    #[error("an unknown error occurred")]
    UnknownError,
    #[error("internal database error: {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
}

post! {
    "/logout",
    #[json_result]
    async fn logout(
        pool: Extension<PgPool>,
        cookies: Cookies,
    ) -> Json<Result<(), LogoutFailure>> {
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
