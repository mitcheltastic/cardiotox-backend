use anyhow::{Context, Result};
use lettre::{
    message::Mailbox,
    transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use tracing::info;
use crate::config::Config;

#[derive(Clone)]
pub struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl Mailer {
    pub fn new(config: &Config) -> Result<Self> {
        let creds = Credentials::new(config.smtp_user.clone(), config.smtp_pass.clone());
        let transport = AsyncSmtpTransport::<Tokio1Executor>::relay(&config.smtp_host)?
            .port(config.smtp_port)
            .credentials(creds)
            .build();
        
        let from: Mailbox = config.email_from.parse().context("Invalid EMAIL_FROM format")?;
        
        Ok(Self { transport, from })
    }

    pub async fn send_verification(&self, to: &str, verify_link: &str) -> Result<()> {
        let to_mailbox: Mailbox = to.parse()?;
        let msg = Message::builder()
            .from(self.from.clone())
            .to(to_mailbox)
            .subject("Verify your email")
            .body(format!("Please verify your email by clicking the link below:\n\n{}", verify_link))?;

        self.transport.send(msg).await?;
        info!("Verification email sent");
        Ok(())
    }

    pub async fn send_reset(&self, to: &str, reset_link: &str) -> Result<()> {
        let to_mailbox: Mailbox = to.parse()?;
        let msg = Message::builder()
            .from(self.from.clone())
            .to(to_mailbox)
            .subject("Password Reset")
            .body(format!("You requested a password reset. Click the link below to reset it:\n\n{}", reset_link))?;

        self.transport.send(msg).await?;
        info!("Password reset email sent");
        Ok(())
    }
}
