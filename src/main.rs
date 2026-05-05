use axum::{Extension, Router, routing::get};
use sqlx::postgres::PgPoolOptions;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

mod db;
mod error;
mod handlers;
mod middleware;
mod state;

use state::AppState;

#[tokio::main]
async fn main() {
    // load .env
    dotenvy::dotenv().ok();

    // logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    // db pool
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");

    let db = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("Failed to connect to Postgres");

    tracing::info!("Connected to Postgres");

    let state = AppState { db };

    let app = Router::new()
        .route("/health", get(handlers::health::health))
        .layer(Extension(state))
        .layer(TraceLayer::new_for_http());

    let addr = "0.0.0.0:8080";
    tracing::info!("Listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
