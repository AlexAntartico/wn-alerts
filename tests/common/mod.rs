#![allow(dead_code)]

pub const RSS_ITEM_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0">
  <channel>
    <title>Azure Status</title>
    <link>https://azure.status.microsoft/en-us/status/</link>
    <description>Azure Status</description>
    <item>
      <title>Azure App Service - Service Disruption</title>
      <description>Customers may experience 503 errors in West US 2.</description>
      <link>https://azure.status.microsoft/en-us/status/incident/int-001</link>
      <guid isPermaLink="false">integ-test-guid-001</guid>
      <pubDate>Wed, 20 May 2026 22:00:00 GMT</pubDate>
    </item>
  </channel>
</rss>"#;

pub fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap()
}
