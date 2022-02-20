use axum::{
    http::StatusCode,
    routing::{get, get_service, post},
    Router,
};
use std::net::SocketAddr;
use tower_cookies::CookieManagerLayer;
use tower_http::{services::ServeDir, trace::TraceLayer};

use marche_server::items::{self, ItemPage, OfferPage, OffersPage, ReactPage};
use marche_server::threads::{self, AuthorPage, Index, ThreadPage};
use marche_server::users::{self, LeaderboardPage, LoginPage, ProfilePage};

#[tokio::main]
async fn main() {
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "marche=info")
    }
    tracing_subscriber::fmt::init();

    let app = Router::new()
        .route("/", get(Index::show))
        .route("/t/*tags", get(Index::show_with_tags))
        .route(
            "/thread/:thread_id",
            get(ThreadPage::show).post(ThreadPage::reply),
        )
        .route(
            "/react/:thread_id",
            get(ReactPage::show).post(ReactPage::apply),
        )
        .route("/remove-tag/:name", post(threads::remove_tag))
        .route("/add-tag", post(threads::add_tag))
        .route("/login", get(LoginPage::show).post(LoginPage::attempt))
        .route("/author", get(AuthorPage::show).post(AuthorPage::publish))
        .route("/profile/:user_id", get(ProfilePage::show))
        .route("/profile", get(users::show_current_user))
        .route("/leaderboard", get(LeaderboardPage::show))
        .route("/item/:item_id", get(ItemPage::show))
        .route("/equip/:item_id", get(items::equip))
        .route("/unequip/:item_id", get(items::unequip))
        .route("/offer/:receiver_id", post(items::make_offer))
        .route("/accept/:trade_id", get(items::accept_offer))
        .route("/decline/:trade_id", get(items::decline_offer))
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
