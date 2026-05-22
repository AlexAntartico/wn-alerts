use super::rss_common::RssProvider;

const ALLOWED_DOMAINS: &[&str] = &[
    "www.cloudflarestatus.com",
    "cloudflarestatus.com",
];
const CONFIG_KEY: &str = "PROVIDER_CLOUDFLARE_FEED_URL";

pub fn new(feed_url: String) -> RssProvider {
    RssProvider::new("cloudflare", feed_url, CONFIG_KEY, ALLOWED_DOMAINS)
}

pub fn new_unvalidated(feed_url: String) -> RssProvider {
    RssProvider::new_unvalidated("cloudflare", feed_url, CONFIG_KEY, ALLOWED_DOMAINS)
}

#[cfg(test)]
mod url_validation_tests {
    use super::*;

    #[test]
    fn validates_allowed_domains() {
        let provider = new("https://www.cloudflarestatus.com/history.rss".into());
        assert!(provider.validate_feed_url().is_ok());

        let provider = new("https://cloudflarestatus.com/history.rss".into());
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
                assert_eq!(key, "PROVIDER_CLOUDFLARE_FEED_URL");
                assert!(value.contains("not in allowed domains"));
            }
            other => panic!("expected InvalidConfig, got {:?}", other),
        }
    }
}
