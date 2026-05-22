pub mod airship;
pub mod aws;
pub mod azure;
pub mod cloudflare;
// pub mod f5;
pub mod github;
pub mod imperva;
// pub mod okta;
pub mod rss_common;
pub mod twilio;

use crate::config::Config;
use crate::core::provider::StatusProvider;
use crate::error::AppError;

pub use crate::core::provider::StatusProvider as StatusProviderTrait;

pub fn build_all(config: &Config) -> Result<Vec<Box<dyn StatusProvider>>, AppError> {
    config
        .enabled_providers
        .iter()
        .map(|name| build_one(name, config))
        .collect()
}

fn build_one(name: &str, _config: &Config) -> Result<Box<dyn StatusProvider>, AppError> {
    match name {
        "azure" => {
            let feed_url = crate::config::provider_param("azure", "FEED_URL")
                .unwrap_or_else(|| "https://azure.status.microsoft/en-us/status/feed/".into());
            Ok(Box::new(azure::new(feed_url)))
        }
        "aws" => {
            let feed_url = crate::config::provider_param("aws", "FEED_URL")
                .unwrap_or_else(|| "https://status.aws.amazon.com/rss/all.rss".into());
            Ok(Box::new(aws::new(feed_url)))
        }
        "twilio" => {
            let feed_url = crate::config::provider_param("twilio", "FEED_URL")
                .unwrap_or_else(|| "https://status.twilio.com/history.rss".into());
            Ok(Box::new(twilio::new(feed_url)))
        }
        "airship" => {
            let feed_url = crate::config::provider_param("airship", "FEED_URL")
                .unwrap_or_else(|| "https://status.airship.com/rss".into());
            Ok(Box::new(airship::new(feed_url)))
        }
        "cloudflare" => {
            let feed_url = crate::config::provider_param("cloudflare", "FEED_URL")
                .unwrap_or_else(|| "https://www.cloudflarestatus.com/history.rss".into());
            Ok(Box::new(cloudflare::new(feed_url)))
        }
        "github" => {
            let feed_url = crate::config::provider_param("github", "FEED_URL")
                .unwrap_or_else(|| "https://www.githubstatus.com/history.rss".into());
            Ok(Box::new(github::new(feed_url)))
        }
        // "okta" => {
        //     let feed_url = crate::config::provider_param("okta", "FEED_URL")
        //         .unwrap_or_else(|| "https://status.okta.com/history.rss".into());
        //     Ok(Box::new(okta::new(feed_url)))
        // }
        "imperva" => {
            let feed_url = crate::config::provider_param("imperva", "FEED_URL")
                .unwrap_or_else(|| "https://status.imperva.com/history.rss".into());
            Ok(Box::new(imperva::new(feed_url)))
        }
        // "f5" => {
        //     let feed_url = crate::config::provider_param("f5", "FEED_URL")
        //         .unwrap_or_else(|| "https://status.f5.com/history.rss".into());
        //     Ok(Box::new(f5::new(feed_url)))
        // }
        unknown => Err(AppError::UnknownProvider(unknown.to_string())),
    }
}
