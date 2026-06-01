use async_trait::async_trait;
use url::Url;

use crate::core::incident::Incident;
use crate::core::provider::StatusProvider;
use crate::error::AppError;

pub const MAX_FEED_SIZE: usize = 10 * 1024 * 1024; // 10 MB

pub struct RssProvider {
    name: &'static str,
    feed_url: String,
    config_key: &'static str,
    allowed_domains: &'static [&'static str],
    skip_validation: bool,
    /// Item `<status>` values to treat as "not an incident" and drop.
    ///
    /// Some feeds (notably `status.cloud.microsoft`) always carry a single
    /// steady-state placeholder item whose `<status>` is `Available` and whose
    /// `pubDate` advances every poll. Left in, it churns the content fingerprint
    /// and re-notifies on every tick. Providers backed by such feeds set this to
    /// the healthy status value(s) so the placeholder is filtered out. Empty for
    /// feeds without a `<status>` element (the default).
    drop_statuses: &'static [&'static str],
}

impl RssProvider {
    pub fn new(
        name: &'static str,
        feed_url: String,
        config_key: &'static str,
        allowed_domains: &'static [&'static str],
    ) -> Self {
        Self {
            name,
            feed_url,
            config_key,
            allowed_domains,
            skip_validation: false,
            drop_statuses: &[],
        }
    }

    pub fn new_unvalidated(
        name: &'static str,
        feed_url: String,
        config_key: &'static str,
        allowed_domains: &'static [&'static str],
    ) -> Self {
        Self {
            name,
            feed_url,
            config_key,
            allowed_domains,
            skip_validation: true,
            drop_statuses: &[],
        }
    }

    /// Drop items whose custom `<status>` element matches (case-insensitively)
    /// one of `statuses`. See [`RssProvider::drop_statuses`] field docs.
    pub fn with_dropped_statuses(mut self, statuses: &'static [&'static str]) -> Self {
        self.drop_statuses = statuses;
        self
    }

    pub fn validate_feed_url(&self) -> Result<(), AppError> {
        if self.skip_validation {
            return Ok(());
        }

        let parsed = Url::parse(&self.feed_url).map_err(|e| AppError::InvalidConfig {
            key: self.config_key,
            value: format!("{} (parse error: {})", self.feed_url, e),
        })?;

        let scheme = parsed.scheme();
        if scheme != "https" && scheme != "http" {
            return Err(AppError::InvalidConfig {
                key: self.config_key,
                value: format!("{} (invalid scheme: {})", self.feed_url, scheme),
            });
        }

        let host = parsed.host_str().ok_or_else(|| AppError::InvalidConfig {
            key: self.config_key,
            value: format!("{} (no host)", self.feed_url),
        })?;

        let is_allowed = self
            .allowed_domains
            .iter()
            .any(|&allowed| host == allowed || host.ends_with(&format!(".{}", allowed)));

        if !is_allowed {
            return Err(AppError::InvalidConfig {
                key: self.config_key,
                value: format!(
                    "{} (host '{}' not in allowed domains: {:?})",
                    self.feed_url, host, self.allowed_domains
                ),
            });
        }

        let is_private_ip = host.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
            std::net::IpAddr::V4(ipv4) => {
                ipv4.is_loopback()
                    || ipv4.is_private()
                    || ipv4.is_link_local()
                    || ipv4.is_unspecified()
                    || ipv4.is_broadcast()
            }
            std::net::IpAddr::V6(ipv6) => {
                ipv6.is_loopback() || ipv6.is_unspecified()
            }
        });

        if is_private_ip {
            return Err(AppError::InvalidConfig {
                key: self.config_key,
                value: format!("{} (private/internal IP not allowed)", self.feed_url),
            });
        }

        Ok(())
    }
}

#[async_trait]
impl StatusProvider for RssProvider {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn check(&self, client: &reqwest::Client) -> Result<Vec<Incident>, AppError> {
        self.validate_feed_url()?;

        let response = client
            .get(&self.feed_url)
            .send()
            .await?
            .error_for_status()?;

        if response.status().is_redirection() {
            return Err(AppError::InvalidConfig {
                key: self.config_key,
                value: format!(
                    "{} returned {} — redirects are not followed",
                    self.feed_url,
                    response.status()
                ),
            });
        }

        if response.content_length().unwrap_or(0) as usize > MAX_FEED_SIZE {
            return Err(AppError::InvalidConfig {
                key: self.config_key,
                value: format!(
                    "{} response too large: {} bytes",
                    self.feed_url,
                    response.content_length().unwrap_or(0)
                ),
            });
        }

        let bytes = response.bytes().await?;
        if bytes.len() > MAX_FEED_SIZE {
            return Err(AppError::InvalidConfig {
                key: self.config_key,
                value: format!("{} response too large: {} bytes", self.feed_url, bytes.len()),
            });
        }

        rss_parser::parse_incidents_from_bytes(&bytes, self.name, self.drop_statuses)
    }
}

mod rss_parser {
    use crate::core::incident::Incident;
    use crate::error::AppError;

    pub fn parse_incidents_from_bytes(
        bytes: &[u8],
        provider: &str,
        drop_statuses: &[&str],
    ) -> Result<Vec<Incident>, AppError> {
        let channel = rss::Channel::read_from(bytes)?;

        // The `rss` crate silently discards unqualified custom elements, so the
        // `<status>` that some feeds attach to each item never reaches us. When a
        // provider asks to drop healthy items, recover a guid -> status map with a
        // targeted scan and use it to filter below.
        let statuses = if drop_statuses.is_empty() {
            std::collections::HashMap::new()
        } else {
            extract_item_statuses(&String::from_utf8_lossy(bytes))
        };

        let incidents: Vec<Incident> = channel
            .items()
            .iter()
            .filter_map(|item| {
                let guid = item.guid()?.value().to_string();
                let title = item.title()?.to_string();

                // Skip non-incident items (e.g. the steady-state "Available"
                // placeholder) whose recovered status is on the drop list.
                if let Some(status) = statuses.get(&guid) {
                    if drop_statuses.iter().any(|d| d.eq_ignore_ascii_case(status)) {
                        return None;
                    }
                }

                Some(Incident {
                    id: guid,
                    provider: provider.to_string(),
                    title,
                    description: item.description().unwrap_or("").to_string(),
                    link: item.link().unwrap_or("").to_string(),
                    occurred_at: item.pub_date().unwrap_or("").to_string(),
                })
            })
            .collect();

        Ok(incidents)
    }

    /// Recovers a `guid -> <status>` map from raw RSS bytes.
    ///
    /// Needed because the `rss` crate drops the unqualified `<status>` element
    /// that `status.cloud.microsoft` feeds attach to every item. Those feeds are
    /// machine-generated and HTML-escape all item bodies (`&lt;`, never a raw
    /// `<`), so splitting on literal `<item>` / `</item>` boundaries is
    /// unambiguous. Items lacking either a `<guid>` or a `<status>` are skipped.
    fn extract_item_statuses(xml: &str) -> std::collections::HashMap<String, String> {
        let mut map = std::collections::HashMap::new();
        for block in xml.split("<item").skip(1) {
            let block = block.split_once("</item>").map_or(block, |(b, _)| b);
            if let (Some(guid), Some(status)) = (inner_text(block, "guid"), inner_text(block, "status")) {
                map.insert(guid, status);
            }
        }
        map
    }

    /// Returns the trimmed inner text of the first `<tag ...>...</tag>` found in
    /// `block`, tolerating attributes on the opening tag (e.g.
    /// `<guid isPermaLink="false">`).
    fn inner_text(block: &str, tag: &str) -> Option<String> {
        let open = block.find(&format!("<{tag}"))?;
        let gt = block[open..].find('>')? + open + 1;
        let close = block[gt..].find(&format!("</{tag}>"))? + gt;
        Some(block[gt..close].trim().to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parse_empty_feed() {
            let xml = br#"<?xml version="1.0"?><rss version="2.0"><channel><title>Test Status</title></channel></rss>"#;
            let incidents = parse_incidents_from_bytes(xml, "test", &[]).unwrap();
            assert!(incidents.is_empty());
        }

        #[test]
        fn parse_feed_with_items() {
            let xml = br#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <title>Test Status</title>
    <item>
      <title>Service Degraded</title>
      <description>Investigating in East US</description>
      <link>https://status.example.com/abc</link>
      <guid>tc-001</guid>
      <pubDate>Thu, 21 May 2026 18:00:00 GMT</pubDate>
    </item>
    <item>
      <title>Service Outage</title>
      <description>West Europe down</description>
      <link>https://status.example.com/def</link>
      <guid>tc-002</guid>
      <pubDate>Thu, 21 May 2026 19:00:00 GMT</pubDate>
    </item>
  </channel>
</rss>"#;

            let incidents = parse_incidents_from_bytes(xml, "test", &[]).unwrap();
            assert_eq!(incidents.len(), 2);
            assert_eq!(incidents[0].id, "tc-001");
            assert_eq!(incidents[0].title, "Service Degraded");
            assert_eq!(incidents[0].provider, "test");
            assert_eq!(incidents[1].id, "tc-002");
            assert_eq!(incidents[1].provider, "test");
        }

        #[test]
        fn parse_item_with_minimal_fields() {
            let xml = br#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <item>
      <title>Minimal</title>
      <guid>min-1</guid>
    </item>
  </channel>
</rss>"#;
            let incidents = parse_incidents_from_bytes(xml, "test", &[]).unwrap();
            assert_eq!(incidents.len(), 1);
            assert_eq!(incidents[0].description, "");
            assert_eq!(incidents[0].link, "");
            assert_eq!(incidents[0].occurred_at, "");
        }

        #[test]
        fn parse_garbage_bytes() {
            assert!(parse_incidents_from_bytes(b"not xml", "test", &[]).is_err());
        }

        // A realistic status.cloud.microsoft feed: one steady-state placeholder
        // whose <status> is Available and whose pubDate tracks the current time.
        const MS_PLACEHOLDER_XML: &[u8] = br#"<?xml version="1.0" encoding="utf-8"?>
<rss xmlns:a10="http://www.w3.org/2005/Atom" version="2.0">
  <channel>
    <title>Microsoft Admin Center Status</title>
    <item>
      <guid isPermaLink="false">a9497817-89ce-4788-b14a-555c2ff9b93b</guid>
      <link>https://status.cloud.microsoft/</link>
      <title>Microsoft Admin Center</title>
      <description>&lt;div&gt;This site is updated when service issues...&lt;/div&gt;</description>
      <pubDate>Mon, 01 Jun 2026 13:15:00 Z</pubDate>
      <status>Available</status>
    </item>
  </channel>
</rss>"#;

        #[test]
        fn drops_items_with_healthy_status() {
            let incidents =
                parse_incidents_from_bytes(MS_PLACEHOLDER_XML, "microsoft_mac", &["Available"]).unwrap();
            assert!(
                incidents.is_empty(),
                "the Available placeholder must be filtered out"
            );
        }

        #[test]
        fn status_filter_is_case_insensitive() {
            let incidents =
                parse_incidents_from_bytes(MS_PLACEHOLDER_XML, "microsoft_mac", &["available"]).unwrap();
            assert!(incidents.is_empty());
        }

        #[test]
        fn keeps_placeholder_when_no_drop_list() {
            // Without a drop list the status is ignored — same as every other feed.
            let incidents = parse_incidents_from_bytes(MS_PLACEHOLDER_XML, "microsoft_mac", &[]).unwrap();
            assert_eq!(incidents.len(), 1);
        }

        #[test]
        fn keeps_non_healthy_status_items() {
            // A genuine incident: status is no longer Available, so it passes through.
            let xml = br#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0">
  <channel>
    <item>
      <guid isPermaLink="false">incident-42</guid>
      <title>Service degradation in West US</title>
      <description>Investigating</description>
      <pubDate>Mon, 01 Jun 2026 14:00:00 Z</pubDate>
      <status>Information</status>
    </item>
  </channel>
</rss>"#;
            let incidents = parse_incidents_from_bytes(xml, "microsoft_mac", &["Available"]).unwrap();
            assert_eq!(incidents.len(), 1);
            assert_eq!(incidents[0].id, "incident-42");
        }

        #[test]
        fn mixed_feed_drops_only_healthy_items() {
            let xml = br#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0">
  <channel>
    <item>
      <guid isPermaLink="false">placeholder</guid>
      <title>Power Platform Admin Center</title>
      <description>boilerplate</description>
      <status>Available</status>
    </item>
    <item>
      <guid isPermaLink="false">real-incident</guid>
      <title>Outage</title>
      <description>Identified</description>
      <status>Warning</status>
    </item>
  </channel>
</rss>"#;
            let incidents = parse_incidents_from_bytes(xml, "microsoft_ppac", &["Available"]).unwrap();
            assert_eq!(incidents.len(), 1);
            assert_eq!(incidents[0].id, "real-incident");
        }

        #[test]
        fn item_without_status_is_kept_even_with_drop_list() {
            // Feeds without a <status> element (every non-MS provider) are unaffected.
            let xml = br#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0">
  <channel>
    <item>
      <guid>no-status</guid>
      <title>Something</title>
    </item>
  </channel>
</rss>"#;
            let incidents = parse_incidents_from_bytes(xml, "test", &["Available"]).unwrap();
            assert_eq!(incidents.len(), 1);
        }
    }
}

#[cfg(test)]
mod url_validation_tests {
    use super::*;

    const TEST_DOMAINS: &[&str] = &["status.example.com", "example.com"];

    fn make_provider(url: &str) -> RssProvider {
        RssProvider::new("test", url.into(), "PROVIDER_TEST_FEED_URL", TEST_DOMAINS)
    }

    #[test]
    fn validates_allowed_domains() {
        let provider = make_provider("https://status.example.com/feed/");
        assert!(provider.validate_feed_url().is_ok());

        let provider = make_provider("https://example.com/feed/");
        assert!(provider.validate_feed_url().is_ok());

        let provider = make_provider("https://sub.example.com/feed/");
        assert!(provider.validate_feed_url().is_ok());
    }

    #[test]
    fn rejects_disallowed_domains() {
        let provider = make_provider("https://evil.example.org/feed/");
        let result = provider.validate_feed_url();
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            AppError::InvalidConfig { key, value } => {
                assert_eq!(key, "PROVIDER_TEST_FEED_URL");
                assert!(value.contains("not in allowed domains"));
            }
            other => panic!("expected InvalidConfig, got {:?}", other),
        }
    }

    #[test]
    fn rejects_private_ips() {
        for url in &[
            "http://127.0.0.1/feed",
            "http://192.168.1.1/feed",
            "http://10.0.0.1/feed",
        ] {
            let provider = make_provider(url);
            assert!(provider.validate_feed_url().is_err());
        }
    }

    #[test]
    fn rejects_invalid_schemes() {
        let provider = make_provider("file:///etc/passwd");
        let result = provider.validate_feed_url();
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            AppError::InvalidConfig { value, .. } => {
                assert!(value.contains("invalid scheme"));
            }
            other => panic!("expected InvalidConfig, got {:?}", other),
        }
    }

    #[test]
    fn rejects_malformed_urls() {
        let provider = make_provider("not a url");
        let result = provider.validate_feed_url();
        assert!(result.is_err());
    }

    #[test]
    fn unvalidated_provider_skips_validation() {
        // A URL that would normally fail domain validation passes when unvalidated.
        let provider = RssProvider::new_unvalidated(
            "test",
            "http://127.0.0.1:8080/feed".into(),
            "PROVIDER_TEST_FEED_URL",
            TEST_DOMAINS,
        );
        assert!(provider.validate_feed_url().is_ok());
    }
}
