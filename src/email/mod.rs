use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::json;
use tracing::{error, info};
use crate::config::Config;

#[derive(Clone)]
pub struct Mailer {
    client: Client,
    api_key: String,
    from: String,
}

impl Mailer {
    pub fn new(config: &Config, client: Client) -> Result<Self> {
        let from = config.email_from.clone();
        if from.is_empty() {
            anyhow::bail!("Invalid EMAIL_FROM format");
        }
        
        Ok(Self { 
            client,
            api_key: config.resend_api_key.clone(),
            from,
        })
    }

    async fn send_resend(&self, to: &str, subject: &str, html: &str) -> Result<()> {
        let payload = json!({
            "from": self.from,
            "to": [to],
            "subject": subject,
            "html": html,
        });

        let res = self.client.post("https://api.resend.com/emails")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await;

        match res {
            Ok(response) => {
                if !response.status().is_success() {
                    let status = response.status();
                    let text = response.text().await.unwrap_or_default();
                    error!("Resend API error: {} - {}", status, text);
                }
            }
            Err(e) => {
                error!("Failed to send email request to Resend: {:?}", e);
            }
        }
        Ok(())
    }

    pub async fn send_verification(&self, to: &str, verify_link: &str) -> Result<()> {
        let subject = "Verify your email";
        let html = format!("Please verify your email by clicking the link below:<br><br><a href=\"{}\">{}</a>", verify_link, verify_link);

        self.send_resend(to, subject, &html).await?;
        info!("Verification email sent");
        Ok(())
    }

    pub async fn send_reset(&self, to: &str, reset_link: &str) -> Result<()> {
        let subject = "Password Reset";
        let html = format!("You requested a password reset. Click the link below to reset it:<br><br><a href=\"{}\">{}</a>", reset_link, reset_link);

        self.send_resend(to, subject, &html).await?;
        info!("Password reset email sent");
        Ok(())
    }
}
