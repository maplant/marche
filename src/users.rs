use chrono::{Duration, prelude::*};
use diesel::prelude::*;
use libpasta::{hash_password, verify_password};
use rocket::form::{Form, FromForm};
use rocket::http::{Cookie, CookieJar, Status};
use rocket::outcome::{try_outcome, IntoOutcome};
use rocket::request::{self, FromRequest};
use rocket::response::Redirect;
use rocket::{uri, Request};
use rocket_dyn_templates::Template;
use serde::Serialize;
use rand::RngCore;
use rand::rngs::OsRng;
use std::collections::HashMap;

#[derive(Queryable, Debug)]
pub struct User {
    /// Id of the user
    pub id: i32,
    /// User name
    pub name: String,
    /// Password hash
    password: String,
    /// Rank
    rank_id: i32,
    /// Last drop
    last_drop: NaiveDateTime,
}

impl User {
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

pub struct UserCache<'a> {
    conn: &'a PgConnection,
    cached: HashMap<i32, User>,
}

impl<'a> UserCache<'a> {
    pub fn new(conn: &'a PgConnection) -> Self {
        UserCache {
            conn,
            cached: Default::default(),
        }
    }

    pub fn get(&mut self, id: i32) -> &User {
        if !self.cached.contains_key(&id) {
            self.cached.insert(id, User::lookup(self.conn, id).unwrap());
        }
        self.cached.get(&id).unwrap()
    }
}

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

const USER_SESSION_ID_COOKIE: &str = "session_id";

/// User login sessions
#[derive(Queryable)]
pub struct LoginSession {
    /// Id of the login session
    id: i32,
    /// Auth token
    session_id: String, 
    /// UserId of the session
    user_id: i32,
    /// When the session began
    session_start: NaiveDateTime,
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
