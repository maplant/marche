use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

use askama::Template;
use axum::{
    async_trait,
    extract::{Form, FromRequest, Path, RequestParts},
    response::{IntoResponse, Redirect, Response},
};
use axum_client_ip::ClientIp;
use chrono::{prelude::*, Duration};
use diesel::prelude::*;
use diesel_derive_enum::DbEnum;
use ipnetwork::IpNetwork;
use libpasta::verify_password;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use tower_cookies::{Cookie, Cookies, Key};

use crate::{
    items::{self, Item, ItemDrop, ItemThumbnail},
    threads::Thread,
    NotFound,
};

table! {
    use diesel::sql_types::*;
    use super::RoleMapping;

    users (id) {
        id -> Integer,
        name -> Text,
        password -> Text,
        bio -> Text,
        role -> RoleMapping,
        experience -> BigInt,
        last_reward -> Timestamp,
        equip_slot_prof_pic -> Nullable<Integer>,
        equip_slot_background -> Nullable<Integer>,
        equip_slot_badges -> Array<Integer>,
    }
}

table! {
    reading_history(id) {
        id -> Integer,
        // It is important that the UNIQUE ( reader_id, article_id )
        // constraint is applied.
        reader_id -> Integer,
        thread_id -> Integer,
        last_read -> Integer,
    }
}

#[derive(Queryable, Debug)]
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
}

pub const MAX_NUM_BADGES: usize = 10;

#[derive(Queryable)]
pub struct ReadingHistory {
    pub id:        i32,
    pub reader_id: i32,
    pub thread_id: i32,
    pub last_read: i32,
}

#[derive(Insertable)]
#[table_name = "reading_history"]
pub struct NewReadingHistory {
    reader_id: i32,
    thread_id: i32,
    last_read: i32,
}

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

    pub fn add_experience(&self, conn: &PgConnection, xp: i64) {
        use self::users::dsl::*;

        // I cannot find a way to do this properly in one sql query.
        // I need to file a bug report or something.

        let user = diesel::update(users.find(self.id))
            .set(experience.eq(experience + xp))
            .get_result::<Self>(conn)
            .unwrap();

        if user.experience < 0 {
            diesel::update(users.find(self.id))
                .set(experience.eq(0))
                .get_result::<Self>(conn)
                .unwrap();
        }
    }

    /// Returns a vec of equipped items.
    pub fn equipped(&self, conn: &PgConnection) -> Vec<ItemDrop> {
        let mut items = Vec::new();

        if let Some(prof_pic) = self.equip_slot_prof_pic {
            items.push(ItemDrop::fetch(conn, prof_pic));
        }

        if let Some(background) = self.equip_slot_background {
            items.push(ItemDrop::fetch(conn, background));
        }

        for badge in self.equip_slot_badges.iter() {
            items.push(ItemDrop::fetch(conn, *badge));
        }

        items
    }

    pub fn inventory(&self, conn: &PgConnection) -> impl Iterator<Item = (Item, ItemDrop)> {
        use crate::items::drops;

        let mut inventory = drops::table
            .filter(drops::dsl::owner_id.eq(self.id))
            .filter(drops::dsl::consumed.eq(false))
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
    fn get_profile_pic(&self, conn: &PgConnection) -> Option<String> {
        self.equip_slot_prof_pic
            .map(|drop_id| ItemDrop::fetch(&conn, drop_id).profile_pic(&conn))
    }

    fn get_background_style(&self, conn: &PgConnection) -> String {
        self.equip_slot_background
            .map(|drop_id| ItemDrop::fetch(&conn, drop_id).background_style(&conn))
            .unwrap_or_else(|| String::from("background: #DBD2E0;"))
    }

    fn get_badges(&self, conn: &PgConnection) -> Vec<String> {
        self.equip_slot_badges
            .iter()
            .map(|drop_id| ItemDrop::fetch(&conn, *drop_id).badge(conn))
            .collect()
    }

    /// Attempt to update the last drop time. If we fail, return false.
    pub fn update_last_reward(&self, conn: &PgConnection) -> Result<Self, ()> {
        use self::users::dsl::{last_reward, users};

        diesel::update(users.find(self.id))
            .set(last_reward.eq(Utc::now().naive_utc()))
            .get_result::<Self>(conn)
            .map_err(|_| ())
    }

    pub fn profile_stub(&self, conn: &PgConnection) -> ProfileStub {
        ProfileStub {
            id:         self.id,
            name:       self.name.clone(),
            picture:    self.get_profile_pic(conn),
            background: self.get_background_style(conn),
            badges:     self.get_badges(conn),
            level:      self.level_info(),
        }
    }

    pub fn fetch(conn: &PgConnection, user_id: i32) -> Result<Self, ()> {
        use self::users::dsl::*;

        users
            .filter(id.eq(user_id))
            .first::<Self>(conn)
            .map_err(|_| ())
    }

    pub fn from_session(conn: &PgConnection, session: &LoginSession) -> Result<Self, ()> {
        use self::users::dsl::*;

        users
            .filter(id.eq(session.user_id))
            .first::<Self>(conn)
            .map_err(|_| ())
    }

    pub fn next_unread(&self, conn: &PgConnection, thread: &Thread) -> i32 {
        let last_read = {
            use self::reading_history::dsl::*;

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
                use crate::threads::{replies::dsl::*, Reply};

                replies
                    .filter(thread_id.eq(thread.id))
                    .order(post_date.asc())
                    .first::<Reply>(conn)
                    .unwrap()
                    .id
            }
            Some(last_read) => {
                use crate::threads::{replies::dsl::*, Reply};

                replies
                    .filter(thread_id.eq(thread.id))
                    .filter(id.gt(last_read))
                    .first::<Reply>(conn)
                    .map_or_else(|_| last_read, |reply| reply.id)
            }
        }
    }

    pub fn has_read(&self, conn: &PgConnection, thread: &Thread) -> bool {
        use self::reading_history::dsl::*;

        reading_history
            .filter(reader_id.eq(self.id))
            .filter(thread_id.eq(thread.id))
            .first::<ReadingHistory>(conn)
            .ok()
            .map_or(false, |history| history.last_read == thread.last_post)
    }

    pub fn read_thread(&self, conn: &PgConnection, thread: &Thread) {
        use self::reading_history::dsl::*;
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
}

#[derive(Template)]
#[template(path = "profile.html")]
pub struct ProfilePage {
    id:           i32,
    name:         String,
    picture:      Option<String>,
    bio:          String,
    level:        LevelInfo,
    equipped:     Vec<ItemThumbnail>,
    inventory:    Vec<ItemThumbnail>,
    badges:       Vec<String>,
    background:   String,
    is_curr_user: bool,
    viewer_role:  Role,
    offers:       i64,
}

impl ProfilePage {
    pub async fn show(curr_user: User, Path(id): Path<i32>) -> Result<Self, NotFound> {
        use crate::items::drops;

        let conn = crate::establish_db_connection();
        let user =
            User::fetch(&conn, id).map_err(|_| NotFound::new(curr_user.incoming_offers(&conn)))?;

        let mut is_equipped = HashSet::new();

        let equipped = user
            .equipped(&conn)
            .into_iter()
            .map(|drop| {
                // TODO: extract this to function.
                is_equipped.insert(drop.id);
                drop.thumbnail(&conn)
            })
            .collect::<Vec<_>>();

        let mut inventory = drops::table
            .filter(drops::dsl::owner_id.eq(user.id))
            .filter(drops::dsl::consumed.eq(false))
            .load::<ItemDrop>(&conn)
            .ok()
            .unwrap_or_else(Vec::new)
            .into_iter()
            .filter_map(|drop| {
                (!is_equipped.contains(&drop.id)).then(|| {
                    let id = drop.item_id;
                    (drop, Item::fetch(&conn, id).rarity)
                })
            })
            .collect::<Vec<_>>();

        inventory.sort_by(|a, b| a.1.cmp(&b.1).reverse());

        let inventory = inventory
            .into_iter()
            .map(|(drop, _)| drop.thumbnail(&conn))
            .collect::<Vec<_>>();

        Ok(Self {
            id,
            picture: user.get_profile_pic(&conn),
            offers: items::IncomingOffer::count(&conn, &curr_user),
            level: user.level_info(),
            badges: user.get_badges(&conn),
            background: user.get_background_style(&conn),
            name: user.name,
            bio: user.bio,
            equipped,
            inventory,
            is_curr_user: user.id == curr_user.id,
            viewer_role: Role::User,
        })
    }
}

pub async fn show_current_user(curr_user: User) -> Redirect {
    Redirect::to(&format!("/profile/{}", curr_user.id))
}

#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq, DbEnum, PartialOrd, Ord)]
pub enum Role {
    User,
    Moderator,
    Admin,
}

/// Displayable user profile
#[derive(Clone, Serialize)]
pub struct ProfileStub {
    pub id:         i32,
    pub name:       String,
    pub picture:    Option<String>,
    pub background: String,
    pub badges:     Vec<String>,
    pub level:      LevelInfo,
}

#[derive(Copy, Clone, Serialize)]
pub struct LevelInfo {
    level:         u32,
    curr_xp:       u64,
    next_level_xp: u64,
}

/// Name of the cookie we use to store the session Id.
const USER_SESSION_ID_COOKIE: &str = "session_id";
const PRIVATE_COOKIE_KEY: &str = "ea63npVp7Vg+ileGuoO0OJbBLOdSkHKkNwu87B8/joU=";

#[async_trait]
impl<B> FromRequest<B> for User
where
    B: Send,
{
    type Rejection = Unauthorized;

    async fn from_request(req: &mut RequestParts<B>) -> Result<Self, Self::Rejection> {
        let cookies = Cookies::from_request(req).await.map_err(|_| Unauthorized)?;
        let private_key = Key::derive_from(PRIVATE_COOKIE_KEY.as_bytes());
        let signed = cookies.private(&private_key);
        let session_id = signed.get(USER_SESSION_ID_COOKIE).ok_or(Unauthorized)?;
        let conn = crate::establish_db_connection();
        let ClientIp(ip) = ClientIp::from_request(req)
            .await
            .map_err(|_| Unauthorized)?;
        let session = LoginSession::fetch(&conn, session_id.value(), IpNetwork::from(ip))
            .map_err(|_| Unauthorized)?;
        User::from_session(&conn, &session).map_err(|_| Unauthorized)
    }
}

pub struct Unauthorized;

impl IntoResponse for Unauthorized {
    fn into_response(self) -> Response {
        Redirect::to("/login").into_response()
    }
}

#[derive(Template)]
#[template(path = "leaderboard.html")]
pub struct LeaderboardPage {
    offers: i64,
    users:  Vec<UserRank>,
}

struct UserRank {
    rank:    usize,
    bio:     String,
    profile: ProfileStub,
}

impl LeaderboardPage {
    pub async fn show(user: User) -> Self {
        use self::users::dsl::*;

        let conn = crate::establish_db_connection();

        let user_profiles = users
            .order(experience.desc())
            .limit(100)
            .load::<User>(&conn)
            .unwrap()
            .into_iter()
            .enumerate()
            .map(|(i, u)| UserRank {
                rank:    i + 1,
                bio:     u.bio.clone(),
                profile: u.profile_stub(&conn),
            })
            .collect();

        Self {
            users:  user_profiles,
            offers: items::IncomingOffer::count(&conn, &user),
        }
    }
}

pub struct UserCache<'a> {
    conn:   &'a PgConnection,
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

table! {
    login_sessions(id) {
        id -> Integer,
        session_id -> Varchar,
        user_id -> Integer,
        session_start -> Timestamp,
        ip_addr -> Cidr,
    }
}

/// User login sessions
// TODO(map): Add the IP address used to create the session for added
// security.
#[derive(Queryable)]
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

#[derive(Insertable)]
#[table_name = "login_sessions"]
pub struct NewSession {
    user_id:       i32,
    session_id:    String,
    session_start: NaiveDateTime,
    ip_addr:       IpNetwork,
}

pub enum LoginFailure {
    UserOrPasswordIncorrect,
    FailedToCreateSession,
    ServerError,
}

impl LoginSession {
    /// Fetch the login session.
    pub fn fetch(conn: &PgConnection, sess_id: &str, ip: IpNetwork) -> Result<Self, ()> {
        use self::login_sessions::dsl::*;

        let curr_session = login_sessions
            .filter(session_id.eq(sess_id))
            .first::<Self>(conn)
            .ok()
            .ok_or(())?;

        // The session is automatically invalid if the session is longer than a month
        // old.
        if curr_session.session_start < (Utc::now() - Duration::weeks(4)).naive_utc() {
            Err(())
        } else {
            Ok(curr_session)
        }
    }

    /// Attempt to login a user
    pub fn login(
        conn: &PgConnection,
        user_name: &str,
        password: &str,
        ip_addr: IpNetwork,
    ) -> Result<Self, LoginFailure> {
        let user_name = user_name.trim();
        let user = {
            use self::users::dsl::*;

            users
                .filter(name.eq(user_name))
                .first::<User>(conn)
                .ok()
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
        let new_session = NewSession {
            user_id: user.id,
            session_id: i128::from_be_bytes(key).to_string(),
            session_start,
            ip_addr,
        };
        diesel::insert_into(login_sessions::table)
            .values(&new_session)
            .get_result(conn)
            .map_err(|_| LoginFailure::FailedToCreateSession)
    }
}

#[derive(Deserialize)]
pub struct LoginForm {
    username: String,
    password: String,
}

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginPage {
    error:  Option<&'static str>,
    offers: usize,
}

impl LoginPage {
    pub async fn show() -> Self {
        Self {
            error:  None,
            offers: 0,
        }
    }

    pub async fn attempt(
        jar: Cookies,
        ClientIp(ip): ClientIp,
        login: Form<LoginForm>,
    ) -> Result<Redirect, Self> {
        let key = Key::derive_from(PRIVATE_COOKIE_KEY.as_bytes());
        let private = jar.private(&key);
        private.remove(Cookie::named(USER_SESSION_ID_COOKIE));
        let conn = crate::establish_db_connection();
        LoginSession::login(&conn, &login.username, &login.password, IpNetwork::from(ip))
            .map(|LoginSession { session_id, .. }| {
                private.add(Cookie::new(USER_SESSION_ID_COOKIE, session_id.to_string()));
                Redirect::to("/")
            })
            .map_err(|_e| Self {
                error:  Some("Incorrect username or password"),
                offers: 0,
            })
    }
}

#[derive(Template)]
#[template(path = "update_bio.html")]
pub struct UpdateBioPage {
    name:       String,
    picture:    Option<String>,
    badges:     Vec<String>,
    background: String,
    bio:        String,
    offers:     usize,
}

#[derive(Deserialize)]
pub struct UpdateBioForm {
    bio: String,
}

impl UpdateBioPage {
    pub async fn show(curr_user: User) -> Self {
        let conn = crate::establish_db_connection();
        Self {
            picture:    curr_user.get_profile_pic(&conn),
            badges:     curr_user.get_badges(&conn),
            background: curr_user.get_background_style(&conn),
            offers:     curr_user.incoming_offers(&conn) as usize,
            bio:        curr_user.bio,
            name:       curr_user.name,
        }
    }

    pub async fn submit(
        curr_user: User,
        Form(UpdateBioForm { bio: new_bio }): Form<UpdateBioForm>,
    ) -> Redirect {
        use self::users::dsl::*;

        let conn = crate::establish_db_connection();
        let _ = diesel::update(users.find(curr_user.id))
            .set(bio.eq(new_bio))
            .get_result::<User>(&conn);

        Redirect::to(&format!("/profile/{}", curr_user.id))
    }
}
