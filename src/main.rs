use std::net::SocketAddr;

use axum::{
    http::StatusCode,
    response::Redirect,
    routing::{get, get_service},
    Router,
};
use marche_server::Endpoint;
use tower_cookies::CookieManagerLayer;
use tower_http::{services::ServeDir, trace::TraceLayer};

#[tokio::main]
async fn main() {
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "marche=info")
    }
    tracing_subscriber::fmt::init();

    // TODO: Use the inventory crate to clean this up.
    let mut app = Router::new();

    for endpoint in inventory::iter::<Endpoint>() {
        app = endpoint.install(app);
    }

    let app = app.route(
        "/favicon.ico",
        get(|| async { Redirect::permanent("/static/favicon.ico") }),
    );

    let app = app
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
