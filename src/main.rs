mod api;
mod auth;
mod config;
mod db;
mod email;
mod error;
mod logging;
mod models;
mod services;
mod state;
mod telemetry;

use axum::{response::IntoResponse, routing::get, Router};
use axum_login::AuthManagerLayerBuilder;
use std::{net::SocketAddr, sync::Arc};
use tower_http::{
    cors::CorsLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};
use axum_login::tower_sessions::{SessionManagerLayer, cookie::SameSite, Expiry};
use tower_sessions_sqlx_store::PostgresStore;
use tower_governor::{governor::GovernorConfigBuilder, GovernorLayer};

use crate::{
    auth::backend::Backend,
    config::Config,
    email::Mailer,
    error::AppError,
    state::AppState,
};

async fn healthz(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    sqlx::query("SELECT 1").execute(&state.db).await?;
    Ok("OK")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    // 1. Initialize telemetry
    telemetry::init();

    // 2. Load configuration
    let config = Config::from_env()?;

    // 3. Setup Database
    let db = db::connect_and_migrate(&config.database_url).await?;

    // 4. Setup Session Store (axum-login requirement)
    let session_store = PostgresStore::new(db.clone());
    session_store.migrate().await?;

    // Cross-site cookie logic
    let mut cookie_secure = config.cookie_secure;
    let samesite = match config.cookie_samesite.as_str() {
        "none" => {
            cookie_secure = true;
            SameSite::None
        }
        _ => SameSite::Lax,
    };

    // Note: Google OAuth top-level redirect works with Lax, but the SPA fetch()-based session
    // needs None+Secure in production (cross-site frontend).
    let session_layer = SessionManagerLayer::new(session_store)
        .with_secure(cookie_secure)
        .with_same_site(samesite)
        .with_expiry(Expiry::OnInactivity(time::Duration::days(7)));

    // 5. Setup AuthManagerLayer
    let backend = Backend { db: db.clone() };
    let auth_layer = AuthManagerLayerBuilder::new(backend, session_layer).build();

    let mailer = Mailer::new(&config)?;

    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    // 6. Build app state
    let state = AppState {
        db: db.clone(),
        config: Arc::new(config.clone()),
        mailer,
        http_client,
    };

    // CSRF Note: We do not add a separate CSRF token. CORS is locked to the exact frontend origin 
    // with credentials. State-changing routes are POST with application/json (forces preflight).
    // The CORS allowlist + JSON-only + credentialed-preflight provides robust protection against CSRF.
    let frontend_origin: axum::http::HeaderValue = config.frontend_origin.parse()?;
    let cors_layer = CorsLayer::new()
        .allow_origin(frontend_origin)
        .allow_credentials(true)
        .allow_methods([axum::http::Method::GET, axum::http::Method::POST])
        .allow_headers([axum::http::header::CONTENT_TYPE]);

    let trace_layer = TraceLayer::new_for_http()
        .make_span_with(|request: &axum::http::Request<_>| {
            let req_id = request
                .headers()
                .get("x-request-id")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown");
            tracing::info_span!(
                "request",
                request_id = req_id,
                method = %request.method(),
                path = %request.uri().path()
            )
        })
        .on_response(tower_http::trace::DefaultOnResponse::new().include_headers(true));

    let middleware = tower::ServiceBuilder::new()
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(trace_layer)
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(cors_layer)
        .layer(auth_layer);

    // 6.5 Rate Limiter
    let governor_conf = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(2)
            .burst_size(10)
            .finish()
            .unwrap(),
    );
    let rate_limit_layer = GovernorLayer::new(governor_conf);

    let api_router = api::router().layer(rate_limit_layer);

    // 7. Build router
    let app = Router::new()
        .nest("/api", api_router)
        .nest("/auth", auth::email_auth::router().merge(auth::google_oauth::router()))
        .route("/healthz", get(healthz))
        .layer(middleware)
        .with_state(state);

    // 8. Start server
    let addr = format!("0.0.0.0:{}", config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("listening on {}", addr);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}
