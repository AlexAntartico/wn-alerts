/// Send a single test notification to your Telegram chat.
/// Uses credentials from .env (NOTIFIER_TELEGRAM_BOT_TOKEN + NOTIFIER_TELEGRAM_CHAT_ID).
///
///   cargo run --example test_notify
use wn_alerts::notifiers::telegram::TelegramNotifier;
use wn_alerts::notifiers::{NotificationKind, Notifier};
use wn_alerts::Incident;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let token = std::env::var("NOTIFIER_TELEGRAM_BOT_TOKEN")
        .expect("NOTIFIER_TELEGRAM_BOT_TOKEN not set");
    let chat_id = std::env::var("NOTIFIER_TELEGRAM_CHAT_ID")
        .expect("NOTIFIER_TELEGRAM_CHAT_ID not set");

    let incident = Incident {
        provider: "twilio".into(),
        id: "test-html-strip-001".into(),
        title: "THIS IS A TEST — HTML stripping check".into(),
        description: concat!(
            "<p><strong>THIS IS A SCHEDULED EVENT Jun ",
            "<var data-var='date'>29</var>, ",
            "<var data-var='time'>06:00</var> - ",
            "<var data-var='time'>07:30</var> PDT</strong></p> ",
            "<p><small>May <var data-var='date'>28</var>, ",
            "<var data-var='time'>19:00</var> PDT</small><br>",
            "<strong>Scheduled</strong> - Twilio will conduct planned maintenance ",
            "on the Japan Twilio edge location. No service disruptions expected.</p>",
        ).into(),
        link: "https://status.twilio.com".into(),
        occurred_at: "Wed, 28 May 2026 19:00:00 GMT".into(),
    };

    let client = reqwest::Client::new();
    let notifier = TelegramNotifier::new(token, chat_id);

    println!("Sending test notification...");
    notifier.notify(&client, &incident, NotificationKind::New).await?;
    println!("Done — check your Telegram chat.");

    Ok(())
}
