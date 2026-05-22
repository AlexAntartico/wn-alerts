use super::rss_common::RssProvider;

const ALLOWED_DOMAINS: &[&str] = &[
    "azure.status.microsoft",
    "status.azure.com",
    "status.microsoft.com",
];
const CONFIG_KEY: &str = "PROVIDER_AZURE_FEED_URL";

pub fn new(feed_url: String) -> RssProvider {
    RssProvider::new("azure", feed_url, CONFIG_KEY, ALLOWED_DOMAINS)
}

pub fn new_unvalidated(feed_url: String) -> RssProvider {
    RssProvider::new_unvalidated("azure", feed_url, CONFIG_KEY, ALLOWED_DOMAINS)
}

#[cfg(test)]
mod url_validation_tests {
    use super::*;

    #[test]
    fn validates_allowed_domains() {
        let provider = new("https://azure.status.microsoft/en-us/status/feed/".into());
        assert!(provider.validate_feed_url().is_ok());

        let provider = new("https://status.azure.com/feed/".into());
        assert!(provider.validate_feed_url().is_ok());

        let provider = new("https://status.microsoft.com/feed/".into());
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
                assert_eq!(key, "PROVIDER_AZURE_FEED_URL");
                assert!(value.contains("not in allowed domains"));
            }
            other => panic!("expected InvalidConfig, got {:?}", other),
        }
    }
}
