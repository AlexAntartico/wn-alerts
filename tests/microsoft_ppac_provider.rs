mod common;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wn_alerts::StatusProvider;

#[tokio::test]
async fn fetches_and_parses_rss() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/feed/ppac"))
        .respond_with(ResponseTemplate::new(200).set_body_string(common::RSS_ITEM_XML))
        .mount(&mock_server)
        .await;

    let feed_url = format!("{}/api/feed/ppac", mock_server.uri());
    let provider = wn_alerts::providers::microsoft_ppac::new_unvalidated(feed_url);

    let incidents = provider.check(&common::build_client()).await.expect("check should succeed");
    assert_eq!(incidents.len(), 1);
    assert_eq!(incidents[0].id, "integ-test-guid-001");
    assert_eq!(incidents[0].provider, "microsoft_ppac");
    assert_eq!(incidents[0].title, "Azure App Service - Service Disruption");
    assert_eq!(
        incidents[0].link,
        "https://azure.status.microsoft/en-us/status/incident/int-001"
    );
}

#[tokio::test]
async fn handles_empty_feed() {
    let mock_server = MockServer::start().await;
    let empty_feed = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0">
  <channel>
    <title>Power Platform Admin Center</title>
    <link>https://status.cloud.microsoft/</link>
    <description>Power Platform Admin Center Status</description>
  </channel>
</rss>"#;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(empty_feed))
        .mount(&mock_server)
        .await;

    let provider = wn_alerts::providers::microsoft_ppac::new_unvalidated(mock_server.uri());
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

    let provider = wn_alerts::providers::microsoft_ppac::new_unvalidated(mock_server.uri());
    assert!(provider.check(&common::build_client()).await.is_err());
}

// The live feed's steady state is a single placeholder item with
// <status>Available</status> and a pubDate that advances every poll. It must
// not be reported as an incident.
#[tokio::test]
async fn drops_available_placeholder() {
    let mock_server = MockServer::start().await;

    let placeholder_feed = r#"<?xml version="1.0" encoding="utf-8"?>
<rss xmlns:a10="http://www.w3.org/2005/Atom" version="2.0">
  <channel>
    <title>Power Platform Admin Center Status</title>
    <link>https://status.cloud.microsoft/</link>
    <description>Power Platform Admin Center Status</description>
    <lastBuildDate>Mon, 01 Jun 2026 13:15:10 Z</lastBuildDate>
    <item>
      <guid isPermaLink="false">9b80edb9-2bb5-47fa-a1d3-8b53dbe65be1</guid>
      <link>https://status.cloud.microsoft/</link>
      <title>Power Platform Admin Center</title>
      <description>&lt;div&gt;This site is updated when service issues are preventing access.&lt;/div&gt;</description>
      <pubDate>Mon, 01 Jun 2026 13:15:00 Z</pubDate>
      <status>Available</status>
    </item>
  </channel>
</rss>"#;

    Mock::given(method("GET"))
        .and(path("/api/feed/ppac"))
        .respond_with(ResponseTemplate::new(200).set_body_string(placeholder_feed))
        .mount(&mock_server)
        .await;

    let feed_url = format!("{}/api/feed/ppac", mock_server.uri());
    let provider = wn_alerts::providers::microsoft_ppac::new_unvalidated(feed_url);

    let incidents = provider.check(&common::build_client()).await.unwrap();
    assert!(incidents.is_empty(), "Available placeholder must be dropped");
}