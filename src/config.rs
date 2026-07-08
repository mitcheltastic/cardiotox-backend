use anyhow::{Context, Result};
use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub port: u16,
    pub cookie_secure: bool,
    pub resend_api_key: String,
    pub email_from: String,
    pub app_base_url: String,
    pub frontend_url: String,
    pub google_client_id: String,
    pub google_client_secret: String,
    pub google_redirect_url: String,
    pub frontend_origin: Vec<String>,
    pub cookie_samesite: String,
    pub hf_space_base: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let database_url = env::var("DATABASE_URL")
            .context("DATABASE_URL must be set in the environment")?;
        
        let port_str = env::var("PORT").unwrap_or_else(|_| "3000".to_string());
        let port: u16 = port_str.parse().context("PORT must be a valid u16 integer")?;

        let cookie_secure = env::var("COOKIE_SECURE")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .unwrap_or(false);

        let resend_api_key = env::var("RESEND_API_KEY").context("RESEND_API_KEY must be set in the environment")?;
        let email_from = env::var("EMAIL_FROM").context("EMAIL_FROM must be set in the environment")?;
        let app_base_url = env::var("APP_BASE_URL").context("APP_BASE_URL must be set in the environment")?;
        let frontend_url = env::var("FRONTEND_URL").context("FRONTEND_URL must be set in the environment")?;
        let google_client_id = env::var("GOOGLE_CLIENT_ID").context("GOOGLE_CLIENT_ID must be set in the environment")?;
        let google_client_secret = env::var("GOOGLE_CLIENT_SECRET").context("GOOGLE_CLIENT_SECRET must be set in the environment")?;
        let google_redirect_url = env::var("GOOGLE_REDIRECT_URL").context("GOOGLE_REDIRECT_URL must be set in the environment")?;
        
        let frontend_origin_raw = env::var("FRONTEND_ORIGIN").context("FRONTEND_ORIGIN must be set in the environment")?;
        let frontend_origin: Vec<String> = frontend_origin_raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        if frontend_origin.is_empty() {
            anyhow::bail!("FRONTEND_ORIGIN must contain at least one valid origin");
        }

        let cookie_samesite = env::var("COOKIE_SAMESITE").unwrap_or_else(|_| "lax".to_string());
        let hf_space_base = env::var("HF_SPACE_BASE").context("HF_SPACE_BASE must be set in the environment")?;

        Ok(Self {
            database_url,
            port,
            cookie_secure,
            resend_api_key,
            email_from,
            app_base_url,
            frontend_url,
            google_client_id,
            google_client_secret,
            google_redirect_url,
            frontend_origin,
            cookie_samesite,
            hf_space_base,
        })
    }

}
