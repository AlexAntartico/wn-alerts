mod common;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wn_alerts::StatusProvider;

#[tokio::test]
async fn rejects_oversized_body() {
    let mock_server = MockServer::start().await;

    // Actual body one byte over the limit; wiremock sets Content-Length to match,
    // so the Content-Length fast-path fires before any body bytes are read.
    let large_body = vec![0u8; wn_alerts::providers::rss_common::MAX_FEED_SIZE + 1];
    Mock::given(method("GET"))
        .and(path("/feed.rss"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(large_body))
        .mount(&mock_server)
        .await;

    let feed_url = format!("{}/feed.rss", mock_server.uri());
    let provider = wn_alerts::providers::azure::new_unvalidated(feed_url);

    let result = provider.check(&common::build_client()).await;
    assert!(result.is_err(), "oversized body should be rejected");
    match result.unwrap_err() {
        wn_alerts::AppError::InvalidConfig { value, .. } => {
            assert!(value.contains("too large"), "error should mention size: {value}");
        }
        other => panic!("expected InvalidConfig, got {:?}", other),
    }
}

#[tokio::test]
async fn rejects_redirect_response() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/feed.rss"))
        .respond_with(
            ResponseTemplate::new(301)
                .insert_header("Location", "http://169.254.169.254/latest/meta-data/"),
        )
        .mount(&mock_server)
        .await;

    let feed_url = format!("{}/feed.rss", mock_server.uri());
    let provider = wn_alerts::providers::azure::new_unvalidated(feed_url);
    // Use the app's real client so Policy::none() is in effect
    let client = wn_alerts::config::ConfigBuilder::new()
        .build()
        .unwrap()
        .build_client()
        .unwrap();

    let result = provider.check(&client).await;
    assert!(result.is_err(), "301 redirect should be rejected");
    match result.unwrap_err() {
        wn_alerts::AppError::InvalidConfig { value, .. } => {
            assert!(value.contains("301"), "error should mention the status code");
            assert!(value.contains("redirect"), "error should mention redirects");
        }
        other => panic!("expected InvalidConfig, got {:?}", other),
    }
}
