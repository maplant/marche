use chrono::prelude::*;
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
use std::collections::HashMap;

#[derive(Queryable, Debug)]
pub struct User {
    /// Id of the user
    id: i32,
    /// User name
    name: String,
    /// Password hash
    password: String,
    /// Rank
    rank_id: i32,
}

impl User {
    pub fn from_session(conn: &PgConnection, session: &LoginSession) -> Result<Self, ()> {
        use crate::schema::users::dsl::*;
        dbg!(users
            .filter(id.eq(session.user_id))
            .load::<Self>(conn)
            .ok()
            .and_then(|v| v.into_iter().next())
            .ok_or(()))
    }
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for User {
    type Error = ();

    async fn from_request(req: &'r Request<'_>) -> request::Outcome<Self, Self::Error> {
        let session_id: i32 = try_outcome!(req
            .cookies()
            .get_private(USER_SESSION_ID_COOKIE)
            .and_then(|x| x.value().parse().ok())
            .or_forward(()));
        let conn = try_outcome!(crate::establish_db_connection().or_forward(()));
        let session = try_outcome!(LoginSession::fetch(&conn, session_id).or_forward(()));
        User::from_session(&conn, &session).or_forward(())
    }
}

const USER_SESSION_ID_COOKIE: &str = "session_id";

/// User login sessions
#[derive(Queryable)]
pub struct LoginSession {
    /// Id of the login session
    id: i32,
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
    session_start: NaiveDateTime,
}

pub enum LoginFailure {
    UserOrPasswordIncorrect,
    FailedToCreateSession,
    ServerError,
}

impl LoginSession {
    /// Get session
    pub fn fetch(conn: &PgConnection, session_id: i32) -> Result<Self, ()> {
        use crate::schema::login_sessions::dsl::*;
        let curr_session = login_sessions
            .filter(id.eq(session_id))
            .load::<Self>(conn)
            .ok()
            .and_then(|v| v.into_iter().next())
            .ok_or(());
        // TODO: Determine if session is older than a month
        curr_session
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

        // TODO: Delete any old login sessions associated with the
        // user.

        // Found a user, create a new login sessions
        let session_start = Utc::now().naive_utc();
        let new_session = NewSession {
            user_id: user.id,
            session_start,
        };
        diesel::insert_into(login_sessions::table)
            .values(&new_session)
            .get_result(conn)
            .map_err(|_| LoginFailure::FailedToCreateSession)
    }
}

#[derive(FromForm, Debug)]
pub struct Login {
    username: String,
    password: String,
}

#[rocket::post("/login", data = "<login>")]
pub fn login_action(jar: &CookieJar<'_>, login: Form<Login>) -> Redirect {
    // let login = login.into_inner();
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
        Ok(LoginSession { id, .. }) => {
            jar.add_private(Cookie::new(USER_SESSION_ID_COOKIE, id.to_string()));
            Redirect::to("/")
        }
        Err(_) => Redirect::to(uri!(crate::error::error("Unauthorized login"))),
    }
}

#[rocket::get("/login")]
pub fn login_form() -> Template {
    Template::render("login", HashMap::<String, String>::new())
}
