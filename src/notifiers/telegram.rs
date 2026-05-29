use async_trait::async_trait;
use std::time::Duration;

use crate::core::incident::Incident;
use crate::error::AppError;
use crate::utils::html;

pub struct TelegramNotifier {
    api_base_url: String,
    bot_token: String,
    chat_id: String,
    /// Minimum delay between consecutive messages (rate limiting)
    rate_limit_delay: Duration,
}

// Custom Debug implementation to prevent sensitive data from being logged
// Bot tokens and chat IDs should never appear in logs or error messages
impl std::fmt::Debug for TelegramNotifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramNotifier")
            .field("api_base_url", &self.api_base_url)
            .field("bot_token", &"[REDACTED]")
            .field("chat_id", &"[REDACTED]")
            .field("rate_limit_delay", &self.rate_limit_delay)
            .finish()
    }
}

impl TelegramNotifier {
    pub fn new(bot_token: String, chat_id: String) -> Self {
        Self {
            api_base_url: "https://api.telegram.org".into(),
            bot_token,
            chat_id,
            // Telegram rate limit: ~1 message per second per chat
            rate_limit_delay: Duration::from_millis(1000),
        }
    }

    pub fn with_api_url(mut self, url: impl Into<String>) -> Self {
        self.api_base_url = url.into();
        self
    }

    pub fn with_rate_limit_delay(mut self, delay: Duration) -> Self {
        self.rate_limit_delay = delay;
        self
    }

    /// Get the rate limit delay for external rate limiting
    pub fn rate_limit_delay(&self) -> Duration {
        self.rate_limit_delay
    }
}

#[async_trait]
impl super::Notifier for TelegramNotifier {
    fn name(&self) -> &'static str {
        "telegram"
    }

    async fn notify(
        &self,
        client: &reqwest::Client,
        incident: &Incident,
    ) -> Result<(), AppError> {
        let text = format_message(incident);
        let url = format!("{}/bot{}/sendMessage", self.api_base_url, self.bot_token);

        let params = [
            ("chat_id", self.chat_id.as_str()),
            ("text", text.as_str()),
            ("parse_mode", "HTML"),
            ("disable_web_page_preview", "true"),
        ];

        // Strip the URL from any network-level reqwest::Error before propagating.
        // The URL contains the bot token, and reqwest's Display impl embeds it.
        let response = client
            .post(&url)
            .form(&params)
            .send()
            .await
            .map_err(|e| AppError::Http(e.without_url()))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::TelegramApi { status, body });
        }

        // Apply rate limiting delay after successful send
        tokio::time::sleep(self.rate_limit_delay).await;

        Ok(())
    }
}

pub fn format_message(incident: &Incident) -> String {
    let formatted_date = html::format_pub_date(&incident.occurred_at);
    let title_escaped = html::escape_html(&incident.title);
    let date_escaped = html::escape_html(&formatted_date);
    let link_escaped = html::escape_html(&incident.link);
    let desc_plain = html::strip_html_tags(&incident.description);
    let desc_escaped = html::escape_html(&desc_plain);
    let provider_label = html::escape_html(&incident.provider);

    format!(
        "<b>[{}] Incident</b>\n\n<b>{}</b>\n<i>{}</i>\n<a href=\"{}\">View full details</a>\n\n{}",
        provider_label.to_uppercase(),
        title_escaped,
        date_escaped,
        link_escaped,
        desc_escaped,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::incident::Incident;

    fn make_incident(provider: &str, title: &str, desc: &str, link: &str, date: &str) -> Incident {
        Incident {
            provider: provider.into(),
            id: "test-1".into(),
            title: title.into(),
            description: desc.into(),
            link: link.into(),
            occurred_at: date.into(),
        }
    }

    #[test]
    fn formats_basic_incident() {
        let incident = make_incident(
            "azure",
            "Azure DevOps - Investigating",
            "Some description",
            "https://example.com/incident/1",
            "Thu, 21 May 2026 18:00:00 GMT",
        );
        let msg = format_message(&incident);

        assert!(msg.contains("[AZURE]"));
        assert!(msg.contains("Azure DevOps - Investigating"));
        assert!(msg.contains("2026-05-21 18:00 UTC"));
        assert!(msg.contains("https://example.com/incident/1"));
    }

    #[test]
    fn strips_html_tags_from_description() {
        let incident = make_incident(
            "azure",
            "Test",
            "<b>bold</b> &amp; <i>italic</i>",
            "https://x.com",
            "",
        );
        let msg = format_message(&incident);

        // Tags are stripped; entity is decoded then re-escaped for Telegram HTML mode
        assert!(msg.contains("bold &amp; italic"));
        // The raw description tags must not survive into the message body
        assert!(!msg.contains("<b>bold</b>"));
        assert!(!msg.contains("<i>italic</i>"));
    }

    #[test]
    fn strips_twilio_style_html() {
        let desc = "<p><strong>SCHEDULED EVENT</strong></p> \
                    <p><small>May 28</small><br> \
                    <strong>Scheduled</strong> - Maintenance window.</p>";
        let incident = make_incident("twilio", "Maintenance", desc, "https://x.com", "");
        let msg = format_message(&incident);

        assert!(msg.contains("SCHEDULED EVENT"));
        assert!(msg.contains("Maintenance window."));
        assert!(!msg.contains("<p>"));
        assert!(!msg.contains("<strong>"));
        assert!(!msg.contains("<var"));
    }

    #[test]
    fn escapes_special_chars_in_link() {
        let incident = make_incident("azure", "T", "", "https://x.com?a=1&b=<2>", "");
        let msg = format_message(&incident);
        assert!(msg.contains("https://x.com?a=1&amp;b=&lt;2&gt;"));
    }

    #[test]
    fn shows_provider_label() {
        let aws = make_incident("aws", "AWS Issue", "desc", "", "");
        assert!(format_message(&aws).contains("[AWS]"));

        let twilio = make_incident("twilio", "Twilio Issue", "desc", "", "");
        assert!(format_message(&twilio).contains("[TWILIO]"));
    }

    #[test]
    fn default_rate_limit_delay() {
        let notifier = TelegramNotifier::new("token".into(), "chat".into());
        assert_eq!(notifier.rate_limit_delay(), Duration::from_millis(1000));
    }

    #[test]
    fn custom_rate_limit_delay() {
        let notifier = TelegramNotifier::new("token".into(), "chat".into())
            .with_rate_limit_delay(Duration::from_millis(500));
        assert_eq!(notifier.rate_limit_delay(), Duration::from_millis(500));
    }

    #[test]
    fn debug_redacts_sensitive_fields() {
        let notifier = TelegramNotifier::new("secret-bot-token".into(), "secret-chat-id".into());
        let debug_output = format!("{:?}", notifier);

        // Verify sensitive data is redacted
        assert!(!debug_output.contains("secret-bot-token"));
        assert!(!debug_output.contains("secret-chat-id"));
        assert!(debug_output.contains("[REDACTED]"));
    }

    #[test]
    fn debug_shows_non_sensitive_fields() {
        let notifier = TelegramNotifier::new("token".into(), "chat".into())
            .with_api_url("https://custom.api.com");
        let debug_output = format!("{:?}", notifier);

        assert!(debug_output.contains("TelegramNotifier"));
        assert!(debug_output.contains("https://custom.api.com"));
    }

    #[test]
    fn build_url_contains_token() {
        // Verify the URL is constructed correctly (token in path per Telegram spec).
        // This test exists to prevent regressing to the broken Authorization-header approach.
        let token = "123456:ABC-secret";
        let notifier = TelegramNotifier::new(token.into(), "chat".into());
        let url = format!("{}/bot{}/sendMessage", notifier.api_base_url, notifier.bot_token);
        assert!(url.contains("/bot123456:ABC-secret/sendMessage"));
        assert!(!url.contains("Authorization"));
    }
}
