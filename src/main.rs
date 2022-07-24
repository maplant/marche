use std::net::SocketAddr;

use axum::{
    http::StatusCode,
    response::Redirect,
    routing::{get, get_service, post},
    Router,
};
use marche_server::{
    items::{self, ItemPage, ReactPage, TradeRequestForm, TradeRequestPage, TradeRequestsPage},
    threads::{
        AuthorPage, EditPostForm, Index, ReplyForm, SetLocked, SetPinned, ThreadForm, ThreadPage,
    },
    users::{self, AddNoteForm, LeaderboardPage, LoginPage, ProfilePage, SetBan, UpdateBioPage},
};
use tower_cookies::CookieManagerLayer;
use tower_http::{services::ServeDir, trace::TraceLayer};

#[tokio::main]
async fn main() {
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "marche=info")
    }
    tracing_subscriber::fmt::init();

    // TODO: Use the inventory crate to clean this up.
    let app = Router::new()
        .route("/", get(Index::show))
        .route(
            "/favicon.ico",
            get(|| async { Redirect::permanent("/static/favicon.ico") }),
        )
        .route("/t/*tags", get(Index::show_with_tags))
        .route(
            "/thread/:thread_id",
            get(ThreadPage::show).post(ReplyForm::submit),
        )
        .route(
            "/react/:thread_id",
            get(ReactPage::show).post(ReactPage::apply),
        )
        .route("/set_pinned", post(SetPinned::set_pinned))
        .route("/set_locked", post(SetLocked::set_locked))
        .route("/edit/:post_id", post(EditPostForm::submit))
        .route("/login", get(LoginPage::show).post(LoginPage::attempt))
        .route("/author", get(AuthorPage::show).post(ThreadForm::submit))
        .route("/profile/:user_id", get(ProfilePage::show))
        .route("/profile", get(users::show_current_user))
        .route(
            "/update_bio",
            get(UpdateBioPage::show).post(UpdateBioPage::submit),
        )
        .route("/leaderboard", get(LeaderboardPage::show))
        .route("/item/:item_id", get(ItemPage::show))
        .route("/equip/:item_id", get(items::equip))
        .route("/unequip/:item_id", get(items::unequip))
        .route("/offer/:receiver_id", post(TradeRequestForm::submit))
        .route("/accept/:trade_id", post(items::accept_offer))
        .route("/decline/:trade_id", post(items::decline_offer))
        .route("/offer/:receiver_id", get(TradeRequestPage::show))
        .route("/offers", get(TradeRequestsPage::show))
        .route("/set_ban/:user_id", post(SetBan::submit))
        .route("/add_note/:user_id", post(AddNoteForm::submit))
        .route("/:catch/*catch", get(marche_server::NotFound::show))
        .nest(
            "/static",
            get_service(ServeDir::new("static")).handle_error(|error: std::io::Error| async move {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Unhandled internal error: {}", error),
                )
            }),
        )
        .layer(CookieManagerLayer::new())
        .layer(TraceLayer::new_for_http());

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    tracing::info!("Marche server launched, listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .unwrap();
}
