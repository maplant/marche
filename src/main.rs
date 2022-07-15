use std::net::SocketAddr;

use axum::{
    http::StatusCode,
    response::Redirect,
    routing::{get, get_service, post},
    Router,
};
use marche_server::{
    items::{self, ItemPage, OfferPage, OffersPage, ReactPage},
    threads::{
        self, AuthorPage, EditPostForm, Index, ReplyForm, SetPinned, ThreadForm, ThreadPage,
    },
    users::{self, LeaderboardPage, LoginPage, ProfilePage, UpdateBioPage},
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
            get(|| async { Redirect::permanent("/static/favicon.ico".parse().unwrap()) }),
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
        .route("/edit/:post_id", post(EditPostForm::submit))
        .route("/remove-tag/:name", post(threads::remove_tag))
        .route("/add-tag", post(threads::add_tag))
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
        .route("/offer/:receiver_id", post(items::make_offer))
        .route("/accept/:trade_id", post(items::accept_offer))
        .route("/decline/:trade_id", post(items::decline_offer))
        .route("/offer/:receiver_id", get(OfferPage::show))
        .route("/offers", get(OffersPage::show))
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
        .serve(app.into_make_service())
        .await
        .unwrap();
}
