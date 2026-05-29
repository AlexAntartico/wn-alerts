use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wn_alerts::notifiers::Notifier;
use wn_alerts::StatusProvider;

const RSS_ITEM_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
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

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap()
}

#[tokio::test]
async fn azure_provider_fetches_and_parses_rss() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/status/feed/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(RSS_ITEM_XML))
        .mount(&mock_server)
        .await;

    let feed_url = format!("{}/status/feed/", mock_server.uri());
    let provider = wn_alerts::providers::azure::new_unvalidated(feed_url);
    let client = build_client();

    let incidents = provider.check(&client).await.expect("check should succeed");
    assert_eq!(incidents.len(), 1);
    assert_eq!(incidents[0].id, "integ-test-guid-001");
    assert_eq!(incidents[0].provider, "azure");
    assert_eq!(incidents[0].title, "Azure App Service - Service Disruption");
    assert_eq!(
        incidents[0].link,
        "https://azure.status.microsoft/en-us/status/incident/int-001"
    );
}

#[tokio::test]
async fn azure_provider_handles_empty_feed() {
    let mock_server = MockServer::start().await;
    let empty_feed = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0">
  <channel>
    <title>Azure Status</title>
    <link>https://azure.status.microsoft/en-us/status/</link>
    <description>Azure Status</description>
  </channel>
</rss>"#;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(empty_feed))
        .mount(&mock_server)
        .await;

    let provider = wn_alerts::providers::azure::new_unvalidated(mock_server.uri());
    let client = build_client();

    let incidents = provider.check(&client).await.unwrap();
    assert!(incidents.is_empty());
}

#[tokio::test]
async fn azure_provider_handles_http_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    let provider = wn_alerts::providers::azure::new_unvalidated(mock_server.uri());
    let client = build_client();
    let result = provider.check(&client).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn telegram_notifier_sends_message() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/bot123:test-token/sendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = reqwest::Client::new();
    let incident = wn_alerts::Incident {
        provider: "azure".into(),
        id: "tg-1".into(),
        title: "Test Incident".into(),
        description: "Test description".into(),
        link: "https://example.com".into(),
        occurred_at: "Thu, 21 May 2026 12:00:00 GMT".into(),
    };

    let notifier = wn_alerts::notifiers::telegram::TelegramNotifier::new(
        "123:test-token".into(),
        "456".into(),
    )
    .with_api_url(mock_server.uri());

    let result = notifier.notify(&client, &incident).await;
    assert!(result.is_ok(), "notify failed: {:?}", result.err());
}

#[tokio::test]
async fn telegram_notifier_handles_api_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/botbad-token/sendMessage"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_string(r#"{"ok":false,"description":"Unauthorized"}"#),
        )
        .mount(&mock_server)
        .await;

    let client = reqwest::Client::new();
    let incident = wn_alerts::Incident {
        provider: "azure".into(),
        id: "tg-fail".into(),
        title: "Test".into(),
        description: "Desc".into(),
        link: "".into(),
        occurred_at: "Thu, 21 May 2026 12:00:00 GMT".into(),
    };

    let notifier = wn_alerts::notifiers::telegram::TelegramNotifier::new(
        "bad-token".into(),
        "456".into(),
    )
    .with_api_url(mock_server.uri());

    let result = notifier.notify(&client, &incident).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        wn_alerts::AppError::TelegramApi { status, .. } => {
            assert_eq!(status, 401);
        }
        other => panic!("expected TelegramApi error, got {:?}", other),
    }
}

#[tokio::test]
async fn twilio_provider_fetches_and_parses_rss() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/history.rss"))
        .respond_with(ResponseTemplate::new(200).set_body_string(RSS_ITEM_XML))
        .mount(&mock_server)
        .await;

    let feed_url = format!("{}/history.rss", mock_server.uri());
    let provider = wn_alerts::providers::twilio::new_unvalidated(feed_url);
    let client = build_client();

    let incidents = provider.check(&client).await.expect("check should succeed");
    assert_eq!(incidents.len(), 1);
    assert_eq!(incidents[0].id, "integ-test-guid-001");
    assert_eq!(incidents[0].provider, "twilio");
}

#[tokio::test]
async fn twilio_provider_handles_empty_feed() {
    let mock_server = MockServer::start().await;
    let empty_feed = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0">
  <channel>
    <title>Twilio Status</title>
    <link>https://status.twilio.com</link>
    <description>Twilio Status</description>
  </channel>
</rss>"#;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(empty_feed))
        .mount(&mock_server)
        .await;

    let provider = wn_alerts::providers::twilio::new_unvalidated(mock_server.uri());
    let client = build_client();
    let incidents = provider.check(&client).await.unwrap();
    assert!(incidents.is_empty());
}

#[tokio::test]
async fn twilio_provider_handles_http_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    let provider = wn_alerts::providers::twilio::new_unvalidated(mock_server.uri());
    let client = build_client();
    let result = provider.check(&client).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn github_provider_fetches_and_parses_rss() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/history.rss"))
        .respond_with(ResponseTemplate::new(200).set_body_string(RSS_ITEM_XML))
        .mount(&mock_server)
        .await;

    let feed_url = format!("{}/history.rss", mock_server.uri());
    let provider = wn_alerts::providers::github::new_unvalidated(feed_url);
    let client = build_client();

    let incidents = provider.check(&client).await.expect("check should succeed");
    assert_eq!(incidents.len(), 1);
    assert_eq!(incidents[0].id, "integ-test-guid-001");
    assert_eq!(incidents[0].provider, "github");
}

#[tokio::test]
async fn github_provider_handles_empty_feed() {
    let mock_server = MockServer::start().await;
    let empty_feed = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0">
  <channel>
    <title>GitHub Status</title>
    <link>https://www.githubstatus.com</link>
    <description>GitHub Status</description>
  </channel>
</rss>"#;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(empty_feed))
        .mount(&mock_server)
        .await;

    let provider =
        wn_alerts::providers::github::new_unvalidated(mock_server.uri());
    let client = build_client();
    let incidents = provider.check(&client).await.unwrap();
    assert!(incidents.is_empty());
}

#[tokio::test]
async fn github_provider_handles_http_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    let provider =
        wn_alerts::providers::github::new_unvalidated(mock_server.uri());
    let client = build_client();
    let result = provider.check(&client).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn cloudflare_provider_fetches_and_parses_rss() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/history.rss"))
        .respond_with(ResponseTemplate::new(200).set_body_string(RSS_ITEM_XML))
        .mount(&mock_server)
        .await;

    let feed_url = format!("{}/history.rss", mock_server.uri());
    let provider = wn_alerts::providers::cloudflare::new_unvalidated(feed_url);
    let client = build_client();

    let incidents = provider.check(&client).await.expect("check should succeed");
    assert_eq!(incidents.len(), 1);
    assert_eq!(incidents[0].id, "integ-test-guid-001");
    assert_eq!(incidents[0].provider, "cloudflare");
}

#[tokio::test]
async fn cloudflare_provider_handles_empty_feed() {
    let mock_server = MockServer::start().await;
    let empty_feed = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0">
  <channel>
    <title>Cloudflare Status</title>
    <link>https://www.cloudflarestatus.com</link>
    <description>Cloudflare Status</description>
  </channel>
</rss>"#;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(empty_feed))
        .mount(&mock_server)
        .await;

    let provider =
        wn_alerts::providers::cloudflare::new_unvalidated(mock_server.uri());
    let client = build_client();
    let incidents = provider.check(&client).await.unwrap();
    assert!(incidents.is_empty());
}

#[tokio::test]
async fn cloudflare_provider_handles_http_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    let provider =
        wn_alerts::providers::cloudflare::new_unvalidated(mock_server.uri());
    let client = build_client();
    let result = provider.check(&client).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn aws_provider_fetches_and_parses_rss() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/rss/all.rss"))
        .respond_with(ResponseTemplate::new(200).set_body_string(RSS_ITEM_XML))
        .mount(&mock_server)
        .await;

    let feed_url = format!("{}/rss/all.rss", mock_server.uri());
    let provider = wn_alerts::providers::aws::new_unvalidated(feed_url);
    let client = build_client();

    let incidents = provider.check(&client).await.expect("check should succeed");
    assert_eq!(incidents.len(), 1);
    assert_eq!(incidents[0].id, "integ-test-guid-001");
    assert_eq!(incidents[0].provider, "aws");
}

#[tokio::test]
async fn aws_provider_handles_empty_feed() {
    let mock_server = MockServer::start().await;
    let empty_feed = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0">
  <channel>
    <title>AWS Service Status</title>
    <link>https://status.aws.amazon.com</link>
    <description>AWS Service Status</description>
  </channel>
</rss>"#;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(empty_feed))
        .mount(&mock_server)
        .await;

    let provider = wn_alerts::providers::aws::new_unvalidated(mock_server.uri());
    let client = build_client();
    let incidents = provider.check(&client).await.unwrap();
    assert!(incidents.is_empty());
}

#[tokio::test]
async fn aws_provider_handles_http_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    let provider = wn_alerts::providers::aws::new_unvalidated(mock_server.uri());
    let client = build_client();
    let result = provider.check(&client).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn airship_provider_fetches_and_parses_rss() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/rss"))
        .respond_with(ResponseTemplate::new(200).set_body_string(RSS_ITEM_XML))
        .mount(&mock_server)
        .await;

    let feed_url = format!("{}/rss", mock_server.uri());
    let provider = wn_alerts::providers::airship::new_unvalidated(feed_url);
    let client = build_client();

    let incidents = provider.check(&client).await.expect("check should succeed");
    assert_eq!(incidents.len(), 1);
    assert_eq!(incidents[0].id, "integ-test-guid-001");
    assert_eq!(incidents[0].provider, "airship");
}

#[tokio::test]
async fn airship_provider_handles_empty_feed() {
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

    let provider =
        wn_alerts::providers::airship::new_unvalidated(mock_server.uri());
    let client = build_client();
    let incidents = provider.check(&client).await.unwrap();
    assert!(incidents.is_empty());
}

#[tokio::test]
async fn airship_provider_handles_http_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    let provider =
        wn_alerts::providers::airship::new_unvalidated(mock_server.uri());
    let client = build_client();
    let result = provider.check(&client).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn imperva_provider_fetches_and_parses_rss() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/history.rss"))
        .respond_with(ResponseTemplate::new(200).set_body_string(RSS_ITEM_XML))
        .mount(&mock_server)
        .await;

    let feed_url = format!("{}/history.rss", mock_server.uri());
    let provider = wn_alerts::providers::imperva::new_unvalidated(feed_url);
    let client = build_client();

    let incidents = provider.check(&client).await.expect("check should succeed");
    assert_eq!(incidents.len(), 1);
    assert_eq!(incidents[0].id, "integ-test-guid-001");
    assert_eq!(incidents[0].provider, "imperva");
}

#[tokio::test]
async fn imperva_provider_handles_empty_feed() {
    let mock_server = MockServer::start().await;
    let empty_feed = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0">
  <channel>
    <title>Imperva Status</title>
    <link>https://status.imperva.com</link>
    <description>Imperva Status</description>
  </channel>
</rss>"#;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(empty_feed))
        .mount(&mock_server)
        .await;

    let provider = wn_alerts::providers::imperva::new_unvalidated(mock_server.uri());
    let client = build_client();
    let incidents = provider.check(&client).await.unwrap();
    assert!(incidents.is_empty());
}

#[tokio::test]
async fn imperva_provider_handles_http_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    let provider = wn_alerts::providers::imperva::new_unvalidated(mock_server.uri());
    let client = build_client();
    let result = provider.check(&client).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn rss_provider_rejects_oversized_body() {
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
    let client = build_client();

    let result = provider.check(&client).await;
    assert!(result.is_err(), "oversized body should be rejected");
    match result.unwrap_err() {
        wn_alerts::AppError::InvalidConfig { value, .. } => {
            assert!(value.contains("too large"), "error should mention size: {value}");
        }
        other => panic!("expected InvalidConfig, got {:?}", other),
    }
}

#[tokio::test]
async fn telegram_notifier_truncates_large_error_body() {
    let mock_server = MockServer::start().await;

    // Error body is 8 KB — double the 4 KB cap — to verify truncation.
    let large_error = "x".repeat(8 * 1024);
    Mock::given(method("POST"))
        .and(path("/botbad-token/sendMessage"))
        .respond_with(ResponseTemplate::new(429).set_body_string(large_error))
        .mount(&mock_server)
        .await;

    let client = reqwest::Client::new();
    let incident = wn_alerts::Incident {
        provider: "azure".into(),
        id: "tg-large-err".into(),
        title: "Test".into(),
        description: "Desc".into(),
        link: "".into(),
        occurred_at: "".into(),
    };
    let notifier = wn_alerts::notifiers::telegram::TelegramNotifier::new(
        "bad-token".into(),
        "456".into(),
    )
    .with_api_url(mock_server.uri());

    let result = notifier.notify(&client, &incident).await;
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

#[tokio::test]
async fn rss_provider_rejects_redirect_response() {
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
