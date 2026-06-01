mod common;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wn_alerts::notifiers::Notifier;

fn make_incident(provider: &str, id: &str) -> wn_alerts::Incident {
    wn_alerts::Incident {
        provider: provider.into(),
        id: id.into(),
        title: "Test Incident".into(),
        description: "Test description".into(),
        link: "https://example.com".into(),
        occurred_at: "Thu, 21 May 2026 12:00:00 GMT".into(),
    }
}

#[tokio::test]
async fn sends_message() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/bot123:test-token/sendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
        .expect(1)
        .mount(&mock_server)
        .await;

    let notifier = wn_alerts::notifiers::telegram::TelegramNotifier::new(
        "123:test-token".into(),
        "456".into(),
    )
    .with_api_url(mock_server.uri());

    let result = notifier
        .notify(
            &reqwest::Client::new(),
            &make_incident("azure", "tg-1"),
            wn_alerts::notifiers::NotificationKind::New,
        )
        .await;
    assert!(result.is_ok(), "notify failed: {:?}", result.err());
}

#[tokio::test]
async fn caps_oversized_message_under_telegram_limit() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/bot123:test-token/sendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
        .expect(1)
        .mount(&mock_server)
        .await;

    // A description far beyond Telegram's 4096-char cap — the bug that made the
    // real API reject with HTTP 400 "message is too long".
    let mut incident = make_incident("github", "tg-oversized");
    incident.description = "A".repeat(10_000);

    let notifier = wn_alerts::notifiers::telegram::TelegramNotifier::new(
        "123:test-token".into(),
        "456".into(),
    )
    .with_api_url(mock_server.uri())
    .with_rate_limit_delay(std::time::Duration::from_millis(0));

    let result = notifier
        .notify(
            &reqwest::Client::new(),
            &incident,
            wn_alerts::notifiers::NotificationKind::New,
        )
        .await;
    assert!(result.is_ok(), "notify failed: {:?}", result.err());

    // Inspect what actually went on the wire: the form-encoded `text` field
    // must be within Telegram's limit.
    let requests = mock_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body = std::str::from_utf8(&requests[0].body).unwrap();
    let text = url::form_urlencoded::parse(body.as_bytes())
        .find(|(k, _)| k == "text")
        .map(|(_, v)| v.into_owned())
        .expect("request body has a `text` field");

    assert!(
        text.chars().count() <= 4096,
        "sent text is {} chars, exceeds Telegram's 4096 limit",
        text.chars().count()
    );
    assert!(text.contains("(truncated)"));
}

#[tokio::test]
async fn handles_api_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/botbad-token/sendMessage"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_string(r#"{"ok":false,"description":"Unauthorized"}"#),
        )
        .mount(&mock_server)
        .await;

    let notifier = wn_alerts::notifiers::telegram::TelegramNotifier::new(
        "bad-token".into(),
        "456".into(),
    )
    .with_api_url(mock_server.uri());

    let result = notifier
        .notify(
            &reqwest::Client::new(),
            &make_incident("azure", "tg-fail"),
            wn_alerts::notifiers::NotificationKind::New,
        )
        .await;
    assert!(result.is_err());
    match result.unwrap_err() {
        wn_alerts::AppError::TelegramApi { status, .. } => assert_eq!(status, 401),
        other => panic!("expected TelegramApi error, got {:?}", other),
    }
}

#[tokio::test]
async fn truncates_large_error_body() {
    let mock_server = MockServer::start().await;

    // Error body is 8 KB — double the 4 KB cap — to verify truncation.
    let large_error = "x".repeat(8 * 1024);
    Mock::given(method("POST"))
        .and(path("/botbad-token/sendMessage"))
        .respond_with(ResponseTemplate::new(429).set_body_string(large_error))
        .mount(&mock_server)
        .await;

    let notifier = wn_alerts::notifiers::telegram::TelegramNotifier::new(
        "bad-token".into(),
        "456".into(),
    )
    .with_api_url(mock_server.uri());

    let result = notifier
        .notify(
            &reqwest::Client::new(),
            &make_incident("azure", "tg-large-err"),
            wn_alerts::notifiers::NotificationKind::New,
        )
        .await;
    assert!(result.is_err());
    match result.unwrap_err() {
        wn_alerts::AppError::TelegramApi { status, body } => {
            assert_eq!(status, 429);
            assert!(
                body.len() <= 4 * 1024,
                "error body should be capped at 4 KB, got {} bytes",
                body.len()
            );
        }
        other => panic!("expected TelegramApi error, got {:?}", other),
    }
}
