// DISABLED: status.okta.com returns 401 — the feed requires authentication.
// Re-enable by: (1) confirming a public unauthenticated feed exists, (2) uncommenting
// `pub mod okta` and the match arm in src/providers/mod.rs.

use super::rss_common::RssProvider;

const ALLOWED_DOMAINS: &[&str] = &[
    "status.okta.com",
];
const CONFIG_KEY: &str = "PROVIDER_OKTA_FEED_URL";

pub fn new(feed_url: String) -> RssProvider {
    RssProvider::new("okta", feed_url, CONFIG_KEY, ALLOWED_DOMAINS)
}

pub fn new_unvalidated(feed_url: String) -> RssProvider {
    RssProvider::new_unvalidated("okta", feed_url, CONFIG_KEY, ALLOWED_DOMAINS)
}

#[cfg(test)]
mod url_validation_tests {
    use super::*;

    #[test]
    fn validates_allowed_domain() {
        let provider = new("https://status.okta.com/history.rss".into());
        assert!(provider.validate_feed_url().is_ok());
    }

    #[test]
    fn rejects_disallowed_domains() {
        let provider = new("https://evil.example.com/feed/".into());
        let result = provider.validate_feed_url();
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            crate::error::AppError::InvalidConfig { key, value } => {
                assert_eq!(key, "PROVIDER_OKTA_FEED_URL");
                assert!(value.contains("not in allowed domains"));
            }
            other => panic!("expected InvalidConfig, got {:?}", other),
        }
    }
}
