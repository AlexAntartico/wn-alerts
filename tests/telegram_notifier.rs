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

    let result = notifier.notify(&reqwest::Client::new(), &make_incident("azure", "tg-1")).await;
    assert!(result.is_ok(), "notify failed: {:?}", result.err());
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

    let result = notifier.notify(&reqwest::Client::new(), &make_incident("azure", "tg-fail")).await;
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
        .notify(&reqwest::Client::new(), &make_incident("azure", "tg-large-err"))
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
