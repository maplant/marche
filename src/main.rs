use axum::error_handling::HandleErrorLayer;
use axum::response::Redirect;
use axum::{
    http::StatusCode,
    routing::{get, get_service, post},
    BoxError, Router,
};
use std::net::SocketAddr;
use tower::ServiceBuilder;
use tower_cookies::{CookieManagerLayer, Key};
use tower_http::{services::ServeDir, trace::TraceLayer};

use marche_server::items::{self, ItemPage, ReactPage, OfferPage};
use marche_server::threads::{AuthorPage, Index, ThreadPage};
use marche_server::users::{LoginPage, ProfilePage};

#[tokio::main]
async fn main() {
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "marche=info")
    }
    tracing_subscriber::fmt::init();

    let app = Router::new()
        .route("/", get(Index::show))
        .route("/thread/:thread_id", get(ThreadPage::show).post(ThreadPage::reply))
        .route("/react/:thread_id", get(ReactPage::show).post(ReactPage::apply))
        .route("/login", get(LoginPage::show).post(LoginPage::attempt))
        .route("/author", get(AuthorPage::show).post(AuthorPage::publish))
        .route("/profile/:user_id", get(ProfilePage::show))
        .route("/profile", get(ProfilePage::show_current))
        .route("/item/:item_id", get(ItemPage::show))
        .route("/equip/:item_id", get(items::equip))
        .route("/unequip/:item_id", get(items::unequip))
        .route("/offer/:receiver_id", get(OfferPage::show))
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
