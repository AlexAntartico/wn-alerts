use super::rss_common::RssProvider;

const ALLOWED_DOMAINS: &[&str] = &["status.cloud.microsoft"];
const CONFIG_KEY: &str = "PROVIDER_MICROSOFT_PPAC_FEED_URL";

pub fn new(feed_url: String) -> RssProvider {
    RssProvider::new("microsoft_ppac", feed_url, CONFIG_KEY, ALLOWED_DOMAINS)
}

pub fn new_unvalidated(feed_url: String) -> RssProvider {
    RssProvider::new_unvalidated("microsoft_ppac", feed_url, CONFIG_KEY, ALLOWED_DOMAINS)
}

#[cfg(test)]
mod url_validation_tests {
    use super::*;

    #[test]
    fn validates_allowed_domains() {
        let provider = new("https://status.cloud.microsoft/api/feed/ppac".into());
        assert!(provider.validate_feed_url().is_ok());
    }

    #[test]
    fn rejects_disallowed_domains() {
        for url in &[
            "https://evil.example.com/feed/",
            "https://status.microsoft.com/feed/",
            "https://arbitrary.status.microsoft.com/feed/",
        ] {
            let provider = new(url.to_string());
            let result = provider.validate_feed_url();
            assert!(result.is_err(), "expected rejection for {url}");
            match result.unwrap_err() {
                crate::error::AppError::InvalidConfig { key, value } => {
                    assert_eq!(key, "PROVIDER_MICROSOFT_PPAC_FEED_URL");
                    assert!(value.contains("not in allowed domains"), "url={url}");
                }
                other => panic!("expected InvalidConfig for {url}, got {:?}", other),
            }
        }
    }
}