mod common;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wn_alerts::StatusProvider;

#[tokio::test]
async fn fetches_and_parses_rss() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/en-us/status/feed/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(MICROSOFT_RSS_XML))
        .expect(1)
        .mount(&mock_server)
        .await;

    let feed_url = format!("{}/en-us/status/feed/", mock_server.uri());
    let provider = wn_alerts::providers::microsoft::new_unvalidated(feed_url);

    let incidents = provider
        .check(&common::build_client())
        .await
        .expect("check should succeed");
    assert_eq!(incidents.len(), 1);
    assert_eq!(incidents[0].id, "ms-integ-guid-001");
    assert_eq!(incidents[0].provider, "microsoft");
    assert_eq!(incidents[0].title, "Microsoft 365 - Service Degradation");
    assert_eq!(
        incidents[0].link,
        "https://status.microsoft.com/en-us/status/incident/ms-001"
    );
}

#[tokio::test]
async fn handles_empty_feed() {
    let mock_server = MockServer::start().await;
    let empty_feed = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0">
  <channel>
    <title>Microsoft Service Status</title>
    <link>https://status.microsoft.com/en-us/status/</link>
    <description>Microsoft Service Status</description>
  </channel>
</rss>"#;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(empty_feed))
        .expect(1)
        .mount(&mock_server)
        .await;

    let provider = wn_alerts::providers::microsoft::new_unvalidated(mock_server.uri());
    let incidents = provider.check(&common::build_client()).await.unwrap();
    assert!(incidents.is_empty());
}

#[tokio::test]
async fn handles_http_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&mock_server)
        .await;

    let provider = wn_alerts::providers::microsoft::new_unvalidated(mock_server.uri());
    assert!(provider.check(&common::build_client()).await.is_err());
}

const MICROSOFT_RSS_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0">
  <channel>
    <title>Microsoft Service Status</title>
    <link>https://status.microsoft.com/en-us/status/</link>
    <description>Microsoft Service Status</description>
    <item>
      <title>Microsoft 365 - Service Degradation</title>
      <description>Users may experience issues accessing Exchange Online.</description>
      <link>https://status.microsoft.com/en-us/status/incident/ms-001</link>
      <guid isPermaLink="false">ms-integ-guid-001</guid>
      <pubDate>Wed, 20 May 2026 22:00:00 GMT</pubDate>
    </item>
  </channel>
</rss>"#;
