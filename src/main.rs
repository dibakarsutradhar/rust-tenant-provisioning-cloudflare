use axum::{
    Extension, Router,
    middleware::{from_fn, from_fn_with_state},
    routing::{get, post},
};
use sqlx::postgres::PgPoolOptions;
use tower_http::{services::ServeDir, trace::TraceLayer};
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
        .route("/api/dashboard", get(handlers::health::tenant_home))
        .route("/api/domains", post(handlers::domains::add_domain))
        .route("/api/domains", get(handlers::domains::list_domains))
        .route_layer(from_fn(middleware::auth::require_auth))
        .route_layer(from_fn_with_state(
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
        )
        .route(
            "/api/domains/stream/:domain_id",
            get(handlers::domains::domain_stream),
        )
        .route(
            "/.well-known/acme-challenge/:token",
            get(handlers::domains::acme_challenge),
        )
        .route(
            "/.well-known/cf-custom-hostname-challenge/:token",
            get(handlers::domains::acme_challenge),
        );

    // root route — serves signup or login depending on subdomain
    let root_route = Router::new().route("/", get(handlers::health::root));

    // static assets — html, css, js files
    // index.html blocked on tenant subdomains via redirect
    let static_files = Router::new()
        .route("/index.html", get(handlers::health::block_signup))
        .fallback_service(ServeDir::new("static"));

    let app = Router::new()
        .merge(public_routes)
        .merge(tenant_routes)
        .merge(root_route)
        .merge(static_files)
        .layer(Extension(state))
        .layer(TraceLayer::new_for_http());

    let addr = "0.0.0.0:8080";
    tracing::info!("Listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
