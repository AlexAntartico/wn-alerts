pub mod telegram;

use async_trait::async_trait;

use crate::config::Config;
use crate::core::incident::Incident;
use crate::error::AppError;

/// Whether a notification is the first alert for an incident or a follow-up
/// triggered by an in-place status update (e.g. Investigating → Identified).
/// Lets notifiers label updates distinctly instead of re-sending an identical
/// "new incident" message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    New,
    Update,
}

#[async_trait]
pub trait Notifier: Send + Sync {
    fn name(&self) -> &'static str;
    async fn notify(
        &self,
        client: &reqwest::Client,
        incident: &Incident,
        kind: NotificationKind,
    ) -> Result<(), AppError>;
}

pub fn build_all(config: &Config) -> Result<Vec<Box<dyn Notifier>>, AppError> {
    config
        .enabled_notifiers
        .iter()
        .map(|name| build_one(name, config))
        .collect()
}

fn build_one(name: &str, _config: &Config) -> Result<Box<dyn Notifier>, AppError> {
    match name {
        "telegram" => {
            let bot_token = crate::config::notifier_param("telegram", "BOT_TOKEN")
                .ok_or(AppError::MissingConfig("NOTIFIER_TELEGRAM_BOT_TOKEN"))?;
            let chat_id = crate::config::notifier_param("telegram", "CHAT_ID")
                .ok_or(AppError::MissingConfig("NOTIFIER_TELEGRAM_CHAT_ID"))?;
            Ok(Box::new(telegram::TelegramNotifier::new(bot_token, chat_id)))
        }
        unknown => Err(AppError::UnknownNotifier(unknown.to_string())),
    }
}
