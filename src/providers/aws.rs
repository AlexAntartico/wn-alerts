use super::rss_common::RssProvider;

const ALLOWED_DOMAINS: &[&str] = &[
    "status.aws.amazon.com",
    "amazonaws.com",
    "aws.amazon.com",
];
const CONFIG_KEY: &str = "PROVIDER_AWS_FEED_URL";

pub fn new(feed_url: String) -> RssProvider {
    RssProvider::new("aws", feed_url, CONFIG_KEY, ALLOWED_DOMAINS)
}

pub fn new_unvalidated(feed_url: String) -> RssProvider {
    RssProvider::new_unvalidated("aws", feed_url, CONFIG_KEY, ALLOWED_DOMAINS)
}

#[cfg(test)]
mod url_validation_tests {
    use super::*;

    #[test]
    fn validates_allowed_domains() {
        let provider = new("https://status.aws.amazon.com/rss/all.rss".into());
        assert!(provider.validate_feed_url().is_ok());

        let provider = new("https://aws.amazon.com/rss/all.rss".into());
        assert!(provider.validate_feed_url().is_ok());

        let provider = new("https://health.aws.amazon.com/rss".into());
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
                assert_eq!(key, "PROVIDER_AWS_FEED_URL");
                assert!(value.contains("not in allowed domains"));
            }
            other => panic!("expected InvalidConfig, got {:?}", other),
        }
    }
}
