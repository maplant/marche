use crate::items::{Item, ItemDrop, ItemType};
use chrono::{prelude::*, Duration};
use diesel::prelude::*;
use libpasta::verify_password;
use rand::rngs::OsRng;
use rand::RngCore;
use rocket::form::{Form, FromForm};
use rocket::http::{Cookie, CookieJar};
use rocket::outcome::{try_outcome, IntoOutcome};
use rocket::request::{self, FromRequest};
use rocket::response::Redirect;
use rocket::{uri, Request};
use rocket_dyn_templates::Template;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

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
    /// Rank
    pub rank_id: i32,
    /// Last reward
    pub last_reward: NaiveDateTime,
    /// ProfilePic equipment slot
    pub equip_slot_prof_pic: Option<i32>,
    /// ProfileBackground equipment slot
    pub equip_slot_background: Option<i32>,
}

impl User {
    /// Equips an item
    pub fn equip(&self, conn: &PgConnection, item_drop: ItemDrop) {
        use crate::schema::users::dsl::*;

        if item_drop.owner_id != self.id {
            return;
        }

        let item_desc = Item::fetch(conn, item_drop.item_id);
        match item_desc.item_type {
            ItemType::ProfilePic { .. } => {
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

    /// Returns the profile picture of the user
    pub fn get_profile_pic(&self, conn: &PgConnection) -> String {
        self.equip_slot_prof_pic
            .map(|drop_id| ItemDrop::fetch(&conn, drop_id).into_profile_pic(&conn))
            .unwrap_or_else(|| String::from("default-prof-pic"))
    }

    pub fn get_background_style(&self, conn: &PgConnection) -> String {
        self.equip_slot_background
            .map(|drop_id| ItemDrop::fetch(&conn, drop_id).into_background_style(&conn))
            .unwrap_or_else(|| String::from(""))
    }

    /// Attempt to update the last drop time. If we fail, return false.
    pub fn update_last_reward(&self, conn: &PgConnection) -> Result<Self, ()> {
        use crate::schema::users::dsl::{last_reward, users};

        diesel::update(users.find(self.id))
            .set(last_reward.eq(Utc::now().naive_utc()))
            .get_result::<Self>(conn)
            .map_err(|_| ())
    }

    pub fn lookup(conn: &PgConnection, user_id: i32) -> Result<Self, ()> {
        use crate::schema::users::dsl::*;
        users
            .filter(id.eq(user_id))
            .load::<Self>(conn)
            .ok()
            .and_then(|v| v.into_iter().next())
            .ok_or(())
    }

    pub fn from_session(conn: &PgConnection, session: &LoginSession) -> Result<Self, ()> {
        use crate::schema::users::dsl::*;
        users
            .filter(id.eq(session.user_id))
            .load::<Self>(conn)
            .ok()
            .and_then(|v| v.into_iter().next())
            .ok_or(())
    }
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
            .or_forward(()));
        let conn = try_outcome!(crate::establish_db_connection().or_forward(()));
        let session = try_outcome!(LoginSession::fetch(&conn, &session_id).or_forward(()));
        User::from_session(&conn, &session).or_forward(())
    }
}

#[rocket::get("/equip/<drop_id>")]
pub fn equip(user: User, drop_id: i32) -> Redirect {
    let conn = crate::establish_db_connection().unwrap();
    let item_drop = ItemDrop::fetch(&conn, drop_id);
    user.equip(&conn, item_drop);
    Redirect::to(uri!(profile(user.id)))
}

#[rocket::get("/profile")]
pub fn curr_profile(user: User) -> Redirect {
    Redirect::to(uri!(profile(user.id)))
}

#[rocket::get("/profile/<id>")]
pub fn profile(_user: User, id: i32) -> Template {
    use crate::schema::drops;

    // TODO: Take this struct and extract it somewhere
    #[derive(Serialize)]
    struct ItemThumb {
        id: i32,
        name: String,
        rarity: String,
        thumbnail: String,
    }

    #[derive(Serialize)]
    struct Context<'n, 'p, 'b, 'bg> {
        name: &'n str,
        picture: &'p str,
        bio: &'b str,
        equipped: Vec<ItemThumb>,
        inventory: Vec<ItemThumb>,
        background: &'bg str,
    }

    let conn = crate::establish_db_connection().unwrap();
    let user = User::lookup(&conn, id).unwrap();

    let mut is_equipped = HashSet::new();

    let equipped = user
        .equipped(&conn)
        .into_iter()
        .map(|drop| {
            // TODO: extract this to function.
            is_equipped.insert(drop.id);
            let item = Item::fetch(&conn, drop.item_id);
            ItemThumb {
                id: drop.id,
                name: item.name,
                rarity: item.rarity.to_string(),
                thumbnail: item.thumbnail,
            }
        })
        .collect::<Vec<_>>();

    let mut inventory = drops::table
        .filter(drops::dsl::owner_id.eq(user.id))
        .load::<ItemDrop>(&conn)
        .ok()
        .unwrap_or_else(Vec::new)
        .into_iter()
        .filter_map(|drop| {
            (!is_equipped.contains(&drop.id)).then(|| (drop.id, Item::fetch(&conn, drop.item_id)))
        })
        .collect::<Vec<_>>();

    inventory.sort_by(|a, b| a.1.rarity.cmp(&b.1.rarity).reverse());

    let inventory = inventory
        .into_iter()
        .map(|(drop_id, item)| ItemThumb {
            id: drop_id,
            name: item.name,
            rarity: item.rarity.to_string(),
            thumbnail: item.thumbnail,
        })
        .collect::<Vec<_>>();

    Template::render(
        "profile",
        Context {
            name: &user.name,
            picture: &user.get_profile_pic(&conn),
            bio: &user.bio,
            equipped,
            inventory,
            background: &user.get_background_style(&conn),
        },
    )
}

pub struct UserCache<'a> {
    conn: &'a PgConnection,
    cached: HashMap<i32, CachedUserData>,
}

pub struct CachedUserData {
    pub user: User,
    pub prof_pic: String,
    pub back_style: String,
}

impl<'a> UserCache<'a> {
    pub fn new(conn: &'a PgConnection) -> Self {
        UserCache {
            conn,
            cached: Default::default(),
        }
    }

    pub fn get(&mut self, id: i32) -> &CachedUserData {
        if !self.cached.contains_key(&id) {
            let user = User::lookup(self.conn, id).unwrap();
            let prof_pic = user.get_profile_pic(self.conn);
            let back_style = user.get_background_style(self.conn);
            self.cached.insert(
                id,
                CachedUserData {
                    user,
                    prof_pic,
                    back_style,
                },
            );
        }
        self.cached.get(&id).unwrap()
    }
}

/// User login sessions
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

use crate::schema::login_sessions;

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
        use crate::schema::login_sessions::dsl::*;
        // TODO: If there are more than two sessions with the same id, fail.
        let curr_session = login_sessions
            .filter(session_id.eq(sess_id))
            .load::<Self>(conn)
            .ok()
            .and_then(|v| v.into_iter().next())
            .ok_or(())?;
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
        let user = {
            use crate::schema::users::dsl::*;
            users
                .filter(name.eq(user_name))
                .load::<User>(conn)
                .ok()
                .and_then(|v| v.into_iter().next())
                .ok_or(LoginFailure::UserOrPasswordIncorrect)?
        };

        if !verify_password(&user.password, password) {
            return Err(LoginFailure::UserOrPasswordIncorrect);
        }

        use crate::schema::login_sessions::dsl::user_id;
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
    let conn = match crate::establish_db_connection() {
        Some(conn) => conn,
        None => {
            return Redirect::to(uri!(crate::error::error(
                "Failed to establish database connection"
            )))
        }
    };
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

/// User privileges. Access to privileges is determined by rank.
pub enum Privilege {
    /// The ability to mint new items. Only the most privileged users users
    /// should have access to this.
    Mint,
}
