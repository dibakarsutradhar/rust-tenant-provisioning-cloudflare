use axum::{
    Extension, Router,
    middleware::{from_fn, from_fn_with_state},
    routing::{get, post},
};
use sqlx::postgres::PgPoolOptions;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

mod db;
mod error;
mod handlers;
mod middleware;
mod services;
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

    let state = AppState::new(db);

    // routes that need tenant context
    let tenant_routes = Router::new()
        .route("/", get(handlers::health::tenant_home))
        .layer(from_fn(middleware::auth::require_auth))
        .layer(from_fn_with_state(
            state.clone(),
            middleware::tenant::resolve_tenant,
        ));

    // public routes — no tenant resolution needed
    let public_routes = Router::new()
        .route("/health", get(handlers::health::health))
        .route("/api/register", post(handlers::auth::register))
        .route("/api/login", post(handlers::auth::login))
        .route(
            "/api/provisioning/stream/:tenant_id",
            get(handlers::provisioning::status_stream),
        );

    let app = Router::new()
        .merge(public_routes)
        .merge(tenant_routes)
        .layer(Extension(state))
        .layer(TraceLayer::new_for_http());

    let addr = "0.0.0.0:8080";
    tracing::info!("Listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
