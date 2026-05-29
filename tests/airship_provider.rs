mod common;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wn_alerts::StatusProvider;

#[tokio::test]
async fn fetches_and_parses_rss() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/rss"))
        .respond_with(ResponseTemplate::new(200).set_body_string(common::RSS_ITEM_XML))
        .mount(&mock_server)
        .await;

    let feed_url = format!("{}/rss", mock_server.uri());
    let provider = wn_alerts::providers::airship::new_unvalidated(feed_url);

    let incidents = provider.check(&common::build_client()).await.expect("check should succeed");
    assert_eq!(incidents.len(), 1);
    assert_eq!(incidents[0].id, "integ-test-guid-001");
    assert_eq!(incidents[0].provider, "airship");
}

#[tokio::test]
async fn handles_empty_feed() {
    let mock_server = MockServer::start().await;
    let empty_feed = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0">
  <channel>
    <title>Airship Status</title>
    <link>https://status.airship.com</link>
    <description>Airship Status</description>
  </channel>
</rss>"#;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(empty_feed))
        .mount(&mock_server)
        .await;

    let provider = wn_alerts::providers::airship::new_unvalidated(mock_server.uri());
    let incidents = provider.check(&common::build_client()).await.unwrap();
    assert!(incidents.is_empty());
}

#[tokio::test]
async fn handles_http_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    let provider = wn_alerts::providers::airship::new_unvalidated(mock_server.uri());
    assert!(provider.check(&common::build_client()).await.is_err());
}
