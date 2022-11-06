use std::{collections::HashMap, ops::Range};

use askama::Template;
use axum::{
    async_trait,
    extract::{Extension, Form, FromRequest, Path, Query, RequestParts},
    response::{IntoResponse, Redirect, Response},
    Json,
};
use axum_client_ip::ClientIp;
use chrono::{prelude::*, Duration};
use derive_more::From;
use ipnetwork::IpNetwork;
use libpasta::verify_password;
use marche_proc_macros::json_result;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgConnection, PgExecutor, PgPool, Type};
use tower_cookies::{Cookie, Cookies, Key};

use crate::post;

use crate::items::ItemDrop;

/*
use crate::{
    items::{self, Item, ItemDrop},
    post,
    threads::Thread,
};
*/

#[derive(FromRow, Debug)]
pub struct User {
    /// Id of the user
    pub id: i32,
    /// User name
    pub name: String,
    /// Password hash
    pub password: String,
    /// Biography of the user
    pub bio: String,
    /// Role
    pub role: Role,
    /// Exprerience
    pub experience: i64,
    /// Last reward
    pub last_reward: NaiveDateTime,
    /// ProfilePic equipment slot
    pub equip_slot_prof_pic: Option<i32>,
    /// ProfileBackground equipment slot
    pub equip_slot_background: Option<i32>,
    /// Badge equipment slots
    pub equip_slot_badges: Vec<i32>,
    /// If the user is banned, and for how long
    pub banned_until: Option<NaiveDateTime>,
    /// Notes on the user by moderators or admins
    pub notes: String,
}

/// Displayable user profile
#[derive(Clone, Serialize)]
pub struct ProfileStub {
    pub id: i32,
    pub name: String,
    pub picture: Option<String>,
    pub background: String,
    pub badges: Vec<String>,
    pub level: LevelInfo,
}

#[derive(
    Copy, Clone, Debug, Hash, Eq, PartialEq, PartialOrd, Ord, Serialize, Deserialize, Type,
)]
#[sqlx(type_name = "role")]
#[sqlx(rename_all = "snake_case")]
pub enum Role {
    User,
    Moderator,
    Admin,
}

#[derive(Copy, Clone, Serialize)]
pub struct LevelInfo {
    pub level: u32,
    pub curr_xp: u64,
    pub next_level_xp: u64,
}

pub const MAX_NUM_BADGES: usize = 10;

impl User {
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
            level: self.level(),
            curr_xp: completion.start,
            next_level_xp: completion.end,
        }
    }

    pub async fn add_experience(&self, conn: &PgPool, xp: i64) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE users SET experience = GREATEST(experience + $1, 0) WHERE id = $2")
            .bind(xp)
            .bind(self.id)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Returns a vec of equipped items.
    pub async fn equipped(&self, conn: &PgPool) -> Result<Vec<ItemDrop>, sqlx::Error> {
        let mut items = Vec::new();

        if let Some(prof_pic) = self.equip_slot_prof_pic {
            ItemDrop::fetch(conn, prof_pic)
                .await?
                .map(|item_drop| items.push(item_drop));
        }

        if let Some(background) = self.equip_slot_background {
            ItemDrop::fetch(conn, background)
                .await?
                .map(|item_drop| items.push(item_drop));
        }

        for badge in self.equip_slot_badges.iter() {
            ItemDrop::fetch(conn, *badge)
                .await?
                .map(|item_drop| items.push(item_drop));
        }

        Ok(items)
    }

    /*
    pub fn inventory(&self, conn: &PgConnection) -> impl Iterator<Item = (Item, ItemDrop)> {
        use crate::schema::drops::dsl::*;

        let mut inventory = drops
            .filter(owner_id.eq(self.id))
            .filter(consumed.eq(false))
            .load::<ItemDrop>(conn)
            .ok()
            .unwrap_or_else(Vec::new)
            .into_iter()
            .map(|drop| {
                let id = drop.item_id;
                (Item::fetch(conn, id), drop)
            })
            .collect::<Vec<_>>();

        inventory.sort_by(|a, b| a.0.rarity.cmp(&b.0.rarity).reverse());

        inventory.into_iter()
    }

    /// Returns the profile picture of the user
    pub fn get_profile_pic(&self, conn: &PgConnection) -> Option<String> {
        self.equip_slot_prof_pic.map(|drop_id| {
            ItemDrop::fetch(&conn, drop_id)
                .unwrap()
                .as_profile_pic(&conn)
        })
    }

    pub fn get_background_style(&self, conn: &PgConnection) -> String {
        self.equip_slot_background
            .map(|drop_id| {
                ItemDrop::fetch(&conn, drop_id)
                    .unwrap()
                    .as_background_style(&conn)
            })
            .unwrap_or_else(|| String::from("background: #ddd;"))
    }

    pub fn get_badges(&self, conn: &PgConnection) -> Vec<String> {
        self.equip_slot_badges
            .iter()
            .map(|drop_id| ItemDrop::fetch(&conn, *drop_id).unwrap().as_badge(conn))
            .collect()
    }
    */

    /// Attempt to update the last drop time. If we fail, return false.
    /// This will fail if the user has received a new reward since the user has
    /// been fetched, which is by design.
    pub async fn update_last_reward(&self, conn: impl PgExecutor<'_>) -> Result<bool, sqlx::Error> {
        let rows_affected =
            sqlx::query("UPDATE users SET last_reward = $1 WHERE id = $2 && last_reward = $3")
                .bind(Utc::now().naive_utc())
                .bind(self.id)
                .bind(self.last_reward)
                .execute(conn)
                .await?
                .rows_affected();
        Ok(rows_affected > 0)
    }

    /*
        pub fn profile_stub(&self, conn: &PgConnection) -> ProfileStub {
            ProfileStub {
                id: self.id,
                name: self.name.clone(),
                picture: self.get_profile_pic(conn),
                background: self.get_background_style(conn),
                badges: self.get_badges(conn),
                level: self.level_info(),
    }
    }
        */

    pub async fn fetch(
        conn: impl PgExecutor<'_>,
        user_id: i32,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(conn)
            .await
    }

    /*
        pub fn from_session(conn: &PgConnection, session: &LoginSession) -> Result<Self, ()> {
            use crate::schema::users::dsl::*;

            users
                .filter(id.eq(session.user_id))
                .first::<Self>(conn)
                .map_err(|_| ())
        }

        pub fn next_unread(&self, conn: &PgConnection, thread: &Thread) -> i32 {
            use crate::schema::replies::dsl::*;
            use crate::threads::Reply;

            let last_read = {
                use crate::schema::reading_history::dsl::*;

                reading_history
                    .filter(reader_id.eq(self.id))
                    .filter(thread_id.eq(thread.id))
                    .first::<ReadingHistory>(conn)
                    .ok()
                    .map(|history| history.last_read)
            };

            match last_read {
                None => {
                    // Find the first reply
                    replies
                        .filter(thread_id.eq(thread.id))
                        .order(post_date.asc())
                        .first::<Reply>(conn)
                        .unwrap()
                        .id
                }
                Some(last_read) => replies
                    .filter(thread_id.eq(thread.id))
                    .filter(id.gt(last_read))
                    .first::<Reply>(conn)
                    .map_or_else(|_| last_read, |reply| reply.id),
            }
        }

        pub fn has_read(&self, conn: &PgConnection, thread: &Thread) -> bool {
            use crate::schema::reading_history::dsl::*;

            reading_history
                .filter(reader_id.eq(self.id))
                .filter(thread_id.eq(thread.id))
                .first::<ReadingHistory>(conn)
                .ok()
                .map_or(false, |history| history.last_read >= thread.last_post)
        }

        pub fn read_thread(&self, conn: &PgConnection, thread: &Thread) {
            use crate::schema::reading_history::dsl::*;

            let _ = diesel::insert_into(reading_history)
                .values(NewReadingHistory {
                    reader_id: self.id,
                    thread_id: thread.id,
                    last_read: thread.last_post,
                })
                .on_conflict((reader_id, thread_id))
                .do_update()
                .set(last_read.eq(thread.last_post))
                .execute(conn);
        }

        pub fn incoming_offers(&self, conn: &PgConnection) -> i64 {
            items::IncomingOffer::count(&conn, self)
    }
        */
}

/*
#[derive(Deserialize)]
pub struct UpdateUser {
    role: Role,
}

#[derive(Serialize, From)]
pub enum UpdateUserError {
    Unprivileged,
    NoSuchUser,
    InternalDbError(#[serde(skip)] diesel::result::Error),
}

post! {
    "/user/:user_id",
    #[json_result]
    async fn update_user(
        pool: Extension<PgPool>,
        moderator: User,
        Path(user_id): Path<i32>,
        Query(UpdateUser { role: new_role }): Query<UpdateUser>
    ) -> Json<Result<(), UpdateUserError>> {
        use crate::schema::users::dsl::*;

        let conn = pool.get().expect("Could not connect to db");
        let user = User::fetch(&conn, user_id).map_err(|_| UpdateUserError::NoSuchUser)?;

        if user.role >= moderator.role || new_role >= moderator.role {
            return Err(UpdateUserError::Unprivileged);
        }

        let _ = diesel::update(users.find(user_id))
            .set(role.eq(new_role))
            .get_result::<User>(&conn)?;

        Ok(())
    }
}

#[derive(Deserialize)]
pub struct BanUser {
    #[serde(default, deserialize_with = "crate::empty_string_as_none")]
    ban_len: Option<u32>,
}

post! {
    "/ban/:user_id",
    #[json_result]
    async fn ban_user(
        pool: Extension<PgPool>,
        moderator: User,
        Path(user_id): Path<i32>,
        Query(BanUser { ban_len }): Query<BanUser>
    ) -> Json<Result<(), UpdateUserError>> {
        use crate::schema::users::dsl::*;

        if moderator.role < Role::Moderator ||  moderator.id == user_id {
            return Err(UpdateUserError::Unprivileged);
        }

        let conn = pool.get().expect("Could not connect to db");
        User::fetch(&conn, user_id).map_err(|_| UpdateUserError::NoSuchUser)?;

        let ban = ban_len.map(|days| (Utc::now() + Duration::days(days as i64)).naive_utc());
            let _ = diesel::update(users.find(user_id))
                .set(banned_until.eq(ban))
                .get_result::<User>(&conn)?;

        Ok(())
    }
}

#[derive(Deserialize)]
pub struct UpdateBioForm {
    bio: String,
}

#[derive(Serialize, From)]
pub enum UpdateBioError {
    TooLong,
    InternalDbError(#[serde(skip)] diesel::result::Error),
}

pub const MAX_BIO_LEN: usize = 300;

post! {
    "/bio",
    #[json_result]
    async fn update_bio(
        pool: Extension<PgPool>,
        curr_user: User,
        Form(UpdateBioForm { bio: new_bio }): Form<UpdateBioForm>,
    ) -> Json<Result<(), UpdateBioError>> {
        use crate::schema::users::dsl::*;

        if new_bio.len() > MAX_BIO_LEN {
            return Err(UpdateBioError::TooLong);
        }

        let conn = pool.get().expect("Could not connect to db");
        diesel::update(users.find(curr_user.id))
            .set(bio.eq(new_bio))
            .get_result::<User>(&conn)?;

        Ok(())
    }
}

#[derive(Deserialize)]
pub struct AddNoteForm {
    body: String,
}

#[derive(Serialize, From)]
pub enum AddNoteError {
    Unprivileged,
    InternalDbError(#[serde(skip)] diesel::result::Error),
}

post! {
    "/add_note/:user_id",
    #[json_result]
    pub async fn submit(
        pool: Extension<PgPool>,
        viewer: User,
        Path(user_id): Path<i32>,
        Form(AddNoteForm { body }): Form<AddNoteForm>,
    ) -> Json<Result<(), AddNoteError>> {
        use crate::schema::users::dsl::*;

        if viewer.role < Role::Moderator {
            return Err(AddNoteError::Unprivileged);
        }

        let conn = pool.get().expect("Could not connect to db");
        let viewer_name = viewer.name;
        let body = html_escape::encode_text(&body);

        let new_note = format!("<p>“{body}” — {viewer_name}</p>");

        let _: User = diesel::update(users.find(user_id))
            .set(notes.eq(notes.concat(new_note)))
            .get_result(&conn)?;

        Ok(())
    }
}

*/

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
        let user = match User::fetch(&*conn, session.user_id).await {
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

#[derive(From)]
pub enum UserRejection {
    UnknownError,
    InternalDbError(sqlx::Error),
    UnknownUser,
    Unauthorized { redirect: String },
    Banned { until: NaiveDateTime },
}

#[derive(Template)]
#[template(path = "banned.html")]
pub struct Banned {
    judge_type: bool,
    until: String,
}

impl IntoResponse for UserRejection {
    fn into_response(self) -> Response {
        match self {
            Self::Banned { until } => Banned {
                judge_type: rand::random(),
                until: until.format(crate::DATE_FMT).to_string(),
            }
            .into_response(),
            Self::Unauthorized { redirect } => {
                Redirect::to(&format!("/login?redirect={redirect}")).into_response()
            }
            _ => todo!(),
        }
    }
}

/*
pub struct UserCache<'a> {
    conn: &'a PgPool,
    cached: HashMap<i32, ProfileStub>,
}

impl<'a> UserCache<'a> {
    pub fn new(conn: &'a PgConnection) -> Self {
        UserCache {
            conn,
            cached: Default::default(),
        }
    }

    pub fn get(&mut self, id: i32) -> &ProfileStub {
        if !self.cached.contains_key(&id) {
            let user = User::fetch(self.conn, id).unwrap();
            self.cached.insert(id, user.profile_stub(self.conn));
        }
        self.cached.get(&id).unwrap()
    }
}
*/

/// User login sessions
#[derive(FromRow)]
pub struct LoginSession {
    /// Id of the login session
    pub id: i32,
    /// Auth token
    pub session_id: String,
    /// UserId of the session
    pub user_id: i32,
    /// When the session began
    pub session_start: NaiveDateTime,
    /// The IP address of the connecting client
    pub ip_addr: IpNetwork,
}

#[derive(Serialize, From)]
pub enum LoginFailure {
    UserOrPasswordIncorrect,
    InternalDbError(#[serde(skip)] sqlx::Error),
}

impl LoginSession {
    /// Fetch the login session.
    pub async fn fetch(conn: &PgPool, session_id: &str) -> Result<Option<Self>, sqlx::Error> {
        Ok(sqlx::query_as("SELECT * FROM login_sessions WHERE id = $1")
            .bind(session_id)
            .fetch_optional(conn)
            .await?
            .filter(|session: &Self| {
                // The session is automatically invalid if the session is longer than a month
                // old.
                session.session_start >= (Utc::now() - Duration::weeks(4)).naive_utc()
            }))
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

#[derive(Serialize, From)]
pub enum LogoutFailure {
    UnknownError,
    InternalDbError(#[serde(skip)] sqlx::Error),
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
    pub id: i32,
    pub reader_id: i32,
    pub thread_id: i32,
    pub last_read: i32,
}
