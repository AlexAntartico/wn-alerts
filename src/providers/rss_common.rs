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
        }
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

        rss_parser::parse_incidents_from_bytes(&bytes, self.name)
    }
}

mod rss_parser {
    use crate::core::incident::Incident;
    use crate::error::AppError;

    pub fn parse_incidents_from_bytes(bytes: &[u8], provider: &str) -> Result<Vec<Incident>, AppError> {
        let channel = rss::Channel::read_from(bytes)?;

        let incidents: Vec<Incident> = channel
            .items()
            .iter()
            .filter_map(|item| {
                let guid = item.guid()?.value().to_string();
                let title = item.title()?.to_string();
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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parse_empty_feed() {
            let xml = br#"<?xml version="1.0"?><rss version="2.0"><channel><title>Test Status</title></channel></rss>"#;
            let incidents = parse_incidents_from_bytes(xml, "test").unwrap();
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

            let incidents = parse_incidents_from_bytes(xml, "test").unwrap();
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
            let incidents = parse_incidents_from_bytes(xml, "test").unwrap();
            assert_eq!(incidents.len(), 1);
            assert_eq!(incidents[0].description, "");
            assert_eq!(incidents[0].link, "");
            assert_eq!(incidents[0].occurred_at, "");
        }

        #[test]
        fn parse_garbage_bytes() {
            assert!(parse_incidents_from_bytes(b"not xml", "test").is_err());
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
