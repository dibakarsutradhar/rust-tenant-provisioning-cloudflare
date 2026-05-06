use axum::{
    Extension, Router,
    middleware::from_fn_with_state,
    routing::{delete, get, post},
};
use sqlx::postgres::PgPoolOptions;
use tower_http::{services::ServeDir, trace::TraceLayer};
use tracing_subscriber::EnvFilter;

mod config;
mod db;
mod error;
mod handlers;
mod middleware;
mod services;
mod state;

use config::Config;
use state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // load configuration
    let config = Config::from_env()?;

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // db pool
    let db = PgPoolOptions::new()
        .max_connections(config.db_max_connections)
        .connect(&config.database_url)
        .await
        .expect("Failed to connect to Postgres");

    tracing::info!("Connected to Postgres");
    tracing::info!("Base domain: {}", config.base_domain);
    tracing::info!("App URL: {}", config.app_base_url());
    tracing::info!("Mock Cloudflare: {}", config.mock_cloudflare);

    let state = AppState::new(db, config.clone());

    // routes that need tenant context
    let tenant_routes = Router::new()
        .route("/api/me", get(handlers::auth::me))
        .route("/api/dashboard", get(handlers::health::tenant_home))
        .route("/api/domains", post(handlers::domains::add_domain))
        .route("/api/domains", get(handlers::domains::list_domains))
        .route("/api/domains/:id", delete(handlers::domains::delete_domain))
        .route(
            "/api/domains/:id/status",
            get(handlers::domains::domain_status),
        )
        .route(
            "/api/domains/:id/verify",
            post(handlers::domains::verify_domain),
        )
        .route_layer(from_fn_with_state(
            state.clone(),
            middleware::auth::require_auth,
        ))
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
        .route("/dashboard.html", get(handlers::health::serve_html))
        .route("/login.html", get(handlers::health::serve_html))
        .route("/status.html", get(handlers::health::serve_html))
        .fallback_service(ServeDir::new("static"));

    let app = Router::new()
        .merge(public_routes)
        .merge(tenant_routes)
        .merge(root_route)
        .merge(static_files)
        .layer(Extension(state))
        .layer(TraceLayer::new_for_http());

    let addr = format!("{}:{}", config.host, config.port);
    tracing::info!("Listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
