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

    async fn send_resend(&self, to: &str, subject: &str, text: &str, html: &str) -> Result<()> {
        let payload = json!({
            "from": self.from,
            "to": [to],
            "subject": subject,
            "text": text,
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
        let text = format!("Please verify your email by clicking or pasting the link below:\n\n{}\n\nThis link will expire in 24 hours.", verify_link);
        let html = verification_html(verify_link);

        self.send_resend(to, subject, &text, &html).await?;
        info!("Verification email sent");
        Ok(())
    }

    pub async fn send_reset(&self, to: &str, reset_link: &str) -> Result<()> {
        let subject = "Password Reset";
        let text = format!("You requested a password reset. Click or paste the link below to reset it:\n\n{}\n\nThis link will expire in 1 hour. If you didn't request this, you can ignore this email.", reset_link);
        let html = reset_html(reset_link);

        self.send_resend(to, subject, &text, &html).await?;
        info!("Password reset email sent");
        Ok(())
    }
}

fn base_html_template(title: &str, text: &str, button_text: &str, link: &str, note: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <meta http-equiv="Content-Type" content="text/html; charset=UTF-8">
</head>
<body style="background-color: #f8fafc; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif; -webkit-font-smoothing: antialiased; font-size: 16px; line-height: 1.5; margin: 0; padding: 0; -ms-text-size-adjust: 100%; -webkit-text-size-adjust: 100%;">
    <table border="0" cellpadding="0" cellspacing="0" class="body" style="border-collapse: separate; mso-table-lspace: 0pt; mso-table-rspace: 0pt; width: 100%; background-color: #f8fafc;">
        <tr>
            <td style="font-family: sans-serif; font-size: 16px; vertical-align: top;">&nbsp;</td>
            <td class="container" style="font-family: sans-serif; font-size: 16px; vertical-align: top; display: block; max-width: 600px; padding: 40px 20px; width: 600px; margin: 0 auto;">
                <div class="content" style="box-sizing: border-box; display: block; margin: 0 auto; max-width: 600px;">
                    <table class="main" style="border-collapse: separate; mso-table-lspace: 0pt; mso-table-rspace: 0pt; width: 100%; background: #ffffff; border-radius: 8px; box-shadow: 0 1px 3px rgba(0,0,0,0.1);">
                        <tr>
                            <td class="wrapper" style="font-family: sans-serif; font-size: 16px; vertical-align: top; box-sizing: border-box; padding: 40px;">
                                <table border="0" cellpadding="0" cellspacing="0" style="border-collapse: separate; mso-table-lspace: 0pt; mso-table-rspace: 0pt; width: 100%;">
                                    <tr>
                                        <td style="font-family: sans-serif; font-size: 16px; vertical-align: top;">
                                            <h2 style="color: #0f172a; font-family: sans-serif; font-weight: 600; line-height: 1.4; margin: 0; margin-bottom: 24px; font-size: 24px; text-align: center; letter-spacing: -0.5px;">Cardiotox</h2>
                                            <h3 style="color: #334155; font-family: sans-serif; font-weight: 500; line-height: 1.4; margin: 0; margin-bottom: 16px; font-size: 18px;">{title}</h3>
                                            <p style="font-family: sans-serif; font-size: 16px; font-weight: normal; margin: 0; margin-bottom: 24px; color: #475569;">{text}</p>
                                            <table border="0" cellpadding="0" cellspacing="0" class="btn btn-primary" style="border-collapse: separate; mso-table-lspace: 0pt; mso-table-rspace: 0pt; width: 100%; box-sizing: border-box;">
                                                <tbody>
                                                    <tr>
                                                        <td align="left" style="font-family: sans-serif; font-size: 16px; vertical-align: top; padding-bottom: 24px;">
                                                            <table border="0" cellpadding="0" cellspacing="0" style="border-collapse: separate; mso-table-lspace: 0pt; mso-table-rspace: 0pt; width: auto;">
                                                                <tbody>
                                                                    <tr>
                                                                        <td style="font-family: sans-serif; font-size: 16px; vertical-align: top; border-radius: 6px; text-align: center; background-color: #2563eb;">
                                                                            <a href="{link}" target="_blank" style="border: solid 1px #2563eb; border-radius: 6px; box-sizing: border-box; cursor: pointer; display: inline-block; font-size: 15px; font-weight: 600; margin: 0; padding: 12px 24px; text-decoration: none; background-color: #2563eb; border-color: #2563eb; color: #ffffff;">{button_text}</a>
                                                                        </td>
                                                                    </tr>
                                                                </tbody>
                                                            </table>
                                                        </td>
                                                    </tr>
                                                </tbody>
                                            </table>
                                            <p style="font-family: sans-serif; font-size: 14px; font-weight: normal; margin: 0; margin-bottom: 16px; color: #64748b;">Or paste this link into your browser:<br><a href="{link}" style="color: #2563eb; text-decoration: none; word-break: break-all;">{link}</a></p>
                                            <p style="font-family: sans-serif; font-size: 13px; font-weight: normal; margin: 0; color: #94a3b8; border-top: 1px solid #e2e8f0; padding-top: 16px; margin-top: 24px;">{note}</p>
                                        </td>
                                    </tr>
                                </table>
                            </td>
                        </tr>
                    </table>
                </div>
            </td>
            <td style="font-family: sans-serif; font-size: 16px; vertical-align: top;">&nbsp;</td>
        </tr>
    </table>
</body>
</html>"#,
        title = title,
        text = text,
        button_text = button_text,
        link = link,
        note = note
    )
}

fn verification_html(link: &str) -> String {
    base_html_template(
        "Verify your email",
        "Thank you for registering. Please click the button below to verify your email address and securely access your account.",
        "Verify email",
        link,
        "This link will expire in 24 hours."
    )
}

fn reset_html(link: &str) -> String {
    base_html_template(
        "Reset your password",
        "You recently requested to reset your password for your Cardiotox account. Click the button below to proceed.",
        "Reset password",
        link,
        "This link will expire in 1 hour. If you didn't request a password reset, you can safely ignore this email."
    )
}
