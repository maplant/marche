use crate::items::{self, Item, ItemDrop, ItemThumbnail, ItemType};
use crate::threads::Thread;
use chrono::{prelude::*, Duration};
use diesel::prelude::*;
use diesel_derive_enum::DbEnum;
use libpasta::verify_password;
use rand::rngs::OsRng;
use rand::RngCore;
use rocket::form::{Form, FromForm};
use rocket::http::{Cookie, CookieJar, Status};
use rocket::outcome::{try_outcome, IntoOutcome};
use rocket::request::{self, FromRequest};
use rocket::response::Redirect;
use rocket::{uri, Request};
use rocket_dyn_templates::Template;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

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
        last_read -> Timestamp,
    }
}

#[derive(Queryable, Debug)]
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
}

#[derive(Queryable)]
pub struct ReadingHistory {
    pub id: i32,
    pub reader_id: i32,
    pub thread_id: i32,
    pub last_read: NaiveDateTime,
}

#[derive(Insertable)]
#[table_name = "reading_history"]
pub struct NewReadingHistory {
    reader_id: i32,
    thread_id: i32,
    last_read: NaiveDateTime,
}

impl User {
    /// Returns the raw, total experience of the user
    pub fn experience(&self) -> u64 {
        self.experience as u64
    }

    /// Returns the level of the user. The level is defined as the log_2 of the user's
    /// experience value.
    pub fn level(&self) -> u32 {
        let xp = self.experience();
        // Base level is 1
        if xp < 4 {
            1
        } else {
            xp.log2()
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
            level: self.level(),
            curr_xp: completion.start,
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

    /// Equips an item
    pub fn equip(&self, conn: &PgConnection, item_drop: ItemDrop) {
        use self::users::dsl::*;

        if item_drop.owner_id != self.id {
            return;
        }

        let item_desc = Item::fetch(conn, item_drop.item_id);
        match item_desc.item_type {
            ItemType::Avatar { .. } => {
                diesel::update(users.find(self.id))
                    .set(equip_slot_prof_pic.eq(Some(item_drop.id)))
                    .get_result::<Self>(conn)
                    .unwrap();
            }
            ItemType::ProfileBackground { .. } => {
                diesel::update(users.find(self.id))
                    .set(equip_slot_background.eq(Some(item_drop.id)))
                    .get_result::<Self>(conn)
                    .unwrap();
            }
            _ => (),
        }
    }

    /// Unequips an item
    pub fn unequip(&self, conn: &PgConnection, item_drop: ItemDrop) {
        use self::users::dsl::*;

        let item_desc = Item::fetch(conn, item_drop.item_id);
        match item_desc.item_type {
            ItemType::Avatar { .. } => {
                let _ = diesel::update(users.find(self.id))
                    .filter(equip_slot_prof_pic.eq(Some(item_drop.id)))
                    .set(equip_slot_prof_pic.eq(Option::<i32>::None))
                    .get_result::<Self>(conn);
            }
            ItemType::ProfileBackground { .. } => {
                let _ = diesel::update(users.find(self.id))
                    .filter(equip_slot_background.eq(Some(item_drop.id)))
                    .set(equip_slot_background.eq(Option::<i32>::None))
                    .get_result::<Self>(conn);
            }
            _ => (),
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

    /// Attempt to update the last drop time. If we fail, return false.
    pub fn update_last_reward(&self, conn: &PgConnection) -> Result<Self, ()> {
        use self::users::dsl::{last_reward, users};

        diesel::update(users.find(self.id))
            .set(last_reward.eq(Utc::now().naive_utc()))
            .get_result::<Self>(conn)
            .map_err(|_| ())
    }

    pub fn profile(&self, conn: &PgConnection) -> UserProfile {
        UserProfile {
            id: self.id,
            name: self.name.clone(),
            picture: self.get_profile_pic(conn),
            background: self.get_background_style(conn),
            level: self.level_info(),
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
}

#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq, DbEnum)]
pub enum Role {
    Admin,
    Moderator,
    User,
}

/// Displayable user profile
#[derive(Clone, Serialize)]
pub struct UserProfile {
    pub id: i32,
    pub name: String,
    pub picture: Option<String>,
    pub background: String,
    pub level: LevelInfo,
}

#[derive(Copy, Clone, Serialize)]
pub struct LevelInfo {
    level: u32,
    curr_xp: u64,
    next_level_xp: u64,
}

/// Name of the cookie we use to store the session Id.
const USER_SESSION_ID_COOKIE: &str = "session_id";

#[rocket::async_trait]
impl<'r> FromRequest<'r> for User {
    type Error = ();

    async fn from_request(req: &'r Request<'_>) -> request::Outcome<Self, Self::Error> {
        let session_id = try_outcome!(req
            .cookies()
            .get_private(USER_SESSION_ID_COOKIE)
            .map(|x| x.value().to_string())
            .into_outcome((Status::Unauthorized, ())));
        let conn = crate::establish_db_connection();
        let session = try_outcome!(
            LoginSession::fetch(&conn, &session_id).into_outcome(Status::Unauthorized)
        );
        User::from_session(&conn, &session).into_outcome(Status::Unauthorized)
    }
}

#[rocket::get("/equip/<drop_id>")]
pub fn equip(user: User, drop_id: i32) -> Redirect {
    let conn = crate::establish_db_connection();
    let item_drop = ItemDrop::fetch(&conn, drop_id);
    user.equip(&conn, item_drop);
    Redirect::to(uri!(profile(user.id)))
}

#[rocket::get("/profile")]
pub fn curr_profile(user: User) -> Redirect {
    Redirect::to(uri!(profile(user.id)))
}

#[rocket::get("/profile/<id>")]
pub fn profile(curr_user: User, id: i32) -> Template {
    use crate::items::drops;

    #[derive(Serialize)]
    struct Context<'n, 'p, 'b, 'bg> {
        id: i32,
        name: &'n str,
        picture: Option<&'p str>,
        bio: &'b str,
        level: LevelInfo,
        equipped: Vec<ItemThumbnail>,
        inventory: Vec<ItemThumbnail>,
        background: &'bg str,
        can_trade: bool,
        offer_count: i64,
    }

    let conn = crate::establish_db_connection();
    let user = User::fetch(&conn, id).unwrap();

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

    Template::render(
        "profile",
        Context {
            id,
            name: &user.name,
            picture: user.get_profile_pic(&conn).as_deref(),
            bio: &user.bio,
            level: user.level_info(),
            equipped,
            inventory,
            background: &user.get_background_style(&conn),
            can_trade: user.id != curr_user.id,
            offer_count: items::IncomingOffer::count(&conn, &curr_user),
        },
    )
}

#[rocket::get("/leaderboard")]
pub fn leaderboard(user: User) -> Template {
    use self::users::dsl::*;

    #[derive(Serialize)]
    struct UserRank {
        rank: usize,
        bio: String, 
        profile: UserProfile,
    }

    #[derive(Serialize)]
    struct Context {
        users: Vec<UserRank>,
        offer_count: i64,
    }

    let conn = crate::establish_db_connection();

    let user_profiles = users
        .order(experience.desc())
        .limit(100)
        .load::<User>(&conn)
        .unwrap()
        .into_iter()
        .enumerate()
        .map(|(i, u)| UserRank {
            rank: i + 1,
            bio: u.bio.clone(),
            profile: u.profile(&conn),
        })
        .collect();

    Template::render(
        "leaderboard",
        Context {
            users: user_profiles,
            offer_count: items::IncomingOffer::count(&conn, &user),
        },
    )
}

pub struct UserCache<'a> {
    conn: &'a PgConnection,
    cached: HashMap<i32, UserProfile>,
}

impl<'a> UserCache<'a> {
    pub fn new(conn: &'a PgConnection) -> Self {
        UserCache {
            conn,
            cached: Default::default(),
        }
    }

    pub fn get(&mut self, id: i32) -> &UserProfile {
        if !self.cached.contains_key(&id) {
            let user = User::fetch(self.conn, id).unwrap();
            self.cached.insert(id, user.profile(self.conn));
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
    }
}

/// User login sessions
// TODO(map): Add the IP address used to create the session for added
// security.
#[derive(Queryable)]
pub struct LoginSession {
    /// Id of the login session
    pub id: i32,
    /// Auth token
    pub session_id: String,
    /// UserId of the session
    pub user_id: i32,
    /// When the session began
    pub session_start: NaiveDateTime,
}

#[derive(Insertable)]
#[table_name = "login_sessions"]
pub struct NewSession {
    user_id: i32,
    session_id: String,
    session_start: NaiveDateTime,
}

pub enum LoginFailure {
    UserOrPasswordIncorrect,
    FailedToCreateSession,
    ServerError,
}

impl LoginSession {
    /// Get session
    pub fn fetch(conn: &PgConnection, sess_id: &str) -> Result<Self, ()> {
        use self::login_sessions::dsl::*;

        // TODO: If there are more than two sessions with the same id, fail.
        let curr_session = login_sessions
            .filter(session_id.eq(sess_id))
            .first::<Self>(conn)
            .ok()
            .ok_or(())?;
        // The session is automatically invalid if the session is longer than a month old.
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
    ) -> Result<Self, LoginFailure> {
        use self::login_sessions::dsl::user_id;

        let user_name = user_name.trim();
        let user = {
            use self::users::dsl::*;

            users
                .filter(name.eq(user_name))
                .first::<User>(conn)
                .ok()
                .ok_or(LoginFailure::UserOrPasswordIncorrect)?
        };
        println!("Got here, {:?}", user);

        if !verify_password(&user.password, password) {
            return Err(LoginFailure::UserOrPasswordIncorrect);
        }

        diesel::delete(login_sessions::table.filter(user_id.eq(user.id)))
            .execute(conn)
            .map_err(|_| LoginFailure::FailedToCreateSession)?;

        let mut key = [0u8; 16];
        OsRng.fill_bytes(&mut key);

        // Found a user, create a new login sessions
        let session_start = Utc::now().naive_utc();
        let new_session = NewSession {
            user_id: user.id,
            session_id: i128::from_be_bytes(key).to_string(),
            session_start,
        };
        diesel::insert_into(login_sessions::table)
            .values(&new_session)
            .get_result(conn)
            .map_err(|_| LoginFailure::FailedToCreateSession)
    }
}

#[derive(FromForm, Debug)]
pub struct LoginReq {
    username: String,
    password: String,
}

#[rocket::post("/login", data = "<login>")]
pub fn login_action(jar: &CookieJar<'_>, login: Form<LoginReq>) -> Redirect {
    jar.remove_private(Cookie::new(USER_SESSION_ID_COOKIE, String::new()));
    let conn = crate::establish_db_connection();
    dbg!(&login.username);
    dbg!(&login.password);
    let session = LoginSession::login(&conn, &login.username, &login.password);
    match session {
        Ok(LoginSession { session_id, .. }) => {
            jar.add_private(Cookie::new(USER_SESSION_ID_COOKIE, session_id.to_string()));
            Redirect::to("/")
        }
        Err(_) => Redirect::to(uri!(crate::error::error("Unauthorized login"))),
    }
}

#[rocket::get("/login")]
pub fn login_form() -> Template {
    Template::render("login", HashMap::<String, String>::new())
}
