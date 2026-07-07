use reqwest::Client;
use sqlx::PgPool;
use std::sync::Arc;
use crate::{config::Config, email::Mailer};

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub config: Arc<Config>,
    pub mailer: Mailer,
    pub http_client: Client,
}
