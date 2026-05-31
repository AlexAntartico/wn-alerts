use crate::error::AppError;

const MAX_POLL_INTERVAL_MINUTES: u64 = 1440; // 24 hours
const MAX_REQUEST_TIMEOUT_SECS: u64 = 300;   // 5 minutes
pub const DEFAULT_BACKOFF_MAX_CYCLES: u32 = 32;

pub struct Config {
    pub poll_interval_minutes: u64,
    pub request_timeout_secs: u64,
    pub state_file_path: String,
    pub enabled_providers: Vec<String>,
    pub enabled_notifiers: Vec<String>,
    /// None = backoff disabled (default). Some(n) = enter backoff after n consecutive failures.
    pub failure_threshold: Option<u32>,
    /// Maximum cycles to skip when backing off (default 32).
    pub backoff_max_cycles: u32,
}

// Custom Debug implementation to redact sensitive information
// when logging (prevents accidental exposure of tokens/secrets)
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("poll_interval_minutes", &self.poll_interval_minutes)
            .field("request_timeout_secs", &self.request_timeout_secs)
            .field("state_file_path", &self.state_file_path)
            .field("enabled_providers", &self.enabled_providers)
            .field("enabled_notifiers", &self.enabled_notifiers)
            .field("failure_threshold", &self.failure_threshold)
            .field("backoff_max_cycles", &self.backoff_max_cycles)
            .finish()
    }
}

pub struct ConfigBuilder {
    poll_interval_minutes: u64,
    request_timeout_secs: u64,
    state_file_path: String,
    enabled_providers: Vec<String>,
    enabled_notifiers: Vec<String>,
    failure_threshold: Option<u32>,
    backoff_max_cycles: u32,
}

impl ConfigBuilder {
    pub fn new() -> Self {
        Self {
            poll_interval_minutes: 5,
            request_timeout_secs: 30,
            state_file_path: "state.json".into(),
            enabled_providers: vec!["azure".into()],
            enabled_notifiers: vec!["telegram".into()],
            failure_threshold: None,
            backoff_max_cycles: DEFAULT_BACKOFF_MAX_CYCLES,
        }
    }

    pub fn poll_interval(mut self, minutes: u64) -> Self {
        self.poll_interval_minutes = minutes;
        self
    }

    pub fn request_timeout(mut self, seconds: u64) -> Self {
        self.request_timeout_secs = seconds;
        self
    }

    pub fn state_file(mut self, path: impl Into<String>) -> Self {
        self.state_file_path = path.into();
        self
    }

    pub fn providers(mut self, names: Vec<String>) -> Self {
        self.enabled_providers = names;
        self
    }

    pub fn notifiers(mut self, names: Vec<String>) -> Self {
        self.enabled_notifiers = names;
        self
    }

    pub fn failure_threshold(mut self, threshold: Option<u32>) -> Self {
        self.failure_threshold = threshold;
        self
    }

    pub fn backoff_max_cycles(mut self, cycles: u32) -> Self {
        self.backoff_max_cycles = cycles;
        self
    }

    pub fn build(self) -> Result<Config, AppError> {
        if self.poll_interval_minutes == 0 {
            return Err(AppError::InvalidConfig {
                key: "POLL_INTERVAL_MINUTES",
                value: "0".into(),
            });
        }
        if self.poll_interval_minutes > MAX_POLL_INTERVAL_MINUTES {
            return Err(AppError::InvalidConfig {
                key: "POLL_INTERVAL_MINUTES",
                value: format!("{} (max {})", self.poll_interval_minutes, MAX_POLL_INTERVAL_MINUTES),
            });
        }
        if self.request_timeout_secs == 0 {
            return Err(AppError::InvalidConfig {
                key: "REQUEST_TIMEOUT_SECS",
                value: "0".into(),
            });
        }
        if self.request_timeout_secs > MAX_REQUEST_TIMEOUT_SECS {
            return Err(AppError::InvalidConfig {
                key: "REQUEST_TIMEOUT_SECS",
                value: format!("{} (max {})", self.request_timeout_secs, MAX_REQUEST_TIMEOUT_SECS),
            });
        }
        if let Some(threshold) = self.failure_threshold {
            if threshold == 0 {
                return Err(AppError::InvalidConfig {
                    key: "PROVIDER_FAILURE_THRESHOLD",
                    value: "0 (must be at least 1)".into(),
                });
            }
        }
        if self.backoff_max_cycles == 0 {
            return Err(AppError::InvalidConfig {
                key: "PROVIDER_BACKOFF_MAX_CYCLES",
                value: "0 (must be at least 1)".into(),
            });
        }

        Ok(Config {
            poll_interval_minutes: self.poll_interval_minutes,
            request_timeout_secs: self.request_timeout_secs,
            state_file_path: self.state_file_path,
            enabled_providers: self.enabled_providers,
            enabled_notifiers: self.enabled_notifiers,
            failure_threshold: self.failure_threshold,
            backoff_max_cycles: self.backoff_max_cycles,
        })
    }
}

impl Default for ConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl Config {
    pub fn from_env() -> Result<Self, AppError> {
        let poll_interval = parse_env_u64("POLL_INTERVAL_MINUTES", "5")?;
        let timeout = parse_env_u64("REQUEST_TIMEOUT_SECS", "30")?;
        let providers = parse_env_list("PROVIDERS", "azure,microsoft");
        let notifiers = parse_env_list("NOTIFIERS", "telegram");
        let state_file_path = env_or_default("STATE_FILE_PATH", "state.json");
        let failure_threshold = parse_env_optional_u32("PROVIDER_FAILURE_THRESHOLD")?;
        let backoff_max_cycles = parse_env_u32(
            "PROVIDER_BACKOFF_MAX_CYCLES",
            &DEFAULT_BACKOFF_MAX_CYCLES.to_string(),
        )?;

        ConfigBuilder::new()
            .poll_interval(poll_interval)
            .request_timeout(timeout)
            .state_file(state_file_path)
            .providers(providers)
            .notifiers(notifiers)
            .failure_threshold(failure_threshold)
            .backoff_max_cycles(backoff_max_cycles)
            .build()
    }

    pub fn build_client(&self) -> Result<reqwest::Client, AppError> {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(self.request_timeout_secs))
            .user_agent(concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| AppError::ClientInit(e.to_string()))
    }
}

pub fn provider_param(provider: &str, key: &str) -> Option<String> {
    let env_key = format!("PROVIDER_{}_{}", provider.to_uppercase(), key);
    std::env::var(&env_key).ok()
}

pub fn notifier_param(notifier: &str, key: &str) -> Option<String> {
    let env_key = format!("NOTIFIER_{}_{}", notifier.to_uppercase(), key);
    std::env::var(&env_key).ok()
}

fn parse_env_list(key: &str, default: &str) -> Vec<String> {
    let raw = env_or_default(key, default);
    if raw.is_empty() {
        return vec![];
    }
    raw.split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

fn env_or_default(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn parse_env_u64(key: &'static str, default: &str) -> Result<u64, AppError> {
    let raw = env_or_default(key, default);
    raw.parse().map_err(|_| AppError::InvalidConfig {
        key,
        value: raw,
    })
}

fn parse_env_u32(key: &'static str, default: &str) -> Result<u32, AppError> {
    let raw = env_or_default(key, default);
    raw.parse().map_err(|_| AppError::InvalidConfig {
        key,
        value: raw,
    })
}

fn parse_env_optional_u32(key: &'static str) -> Result<Option<u32>, AppError> {
    match std::env::var(key) {
        Ok(raw) => raw.parse::<u32>().map(Some).map_err(|_| AppError::InvalidConfig {
            key,
            value: raw,
        }),
        Err(_) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_defaults() {
        let config = ConfigBuilder::new().build().unwrap();
        assert_eq!(config.poll_interval_minutes, 5);
        assert_eq!(config.request_timeout_secs, 30);
        assert_eq!(config.state_file_path, "state.json");
        assert_eq!(config.enabled_providers, vec!["azure"]);
        assert_eq!(config.enabled_notifiers, vec!["telegram"]);
        assert_eq!(config.failure_threshold, None);
        assert_eq!(config.backoff_max_cycles, DEFAULT_BACKOFF_MAX_CYCLES);
    }

    #[test]
    fn builder_custom_values() {
        let config = ConfigBuilder::new()
            .poll_interval(10)
            .request_timeout(45)
            .providers(vec!["azure".into(), "aws".into()])
            .notifiers(vec!["telegram".into()])
            .build()
            .unwrap();

        assert_eq!(config.poll_interval_minutes, 10);
        assert_eq!(config.request_timeout_secs, 45);
        assert_eq!(config.enabled_providers, vec!["azure", "aws"]);
    }

    #[test]
    fn builder_rejects_zero_poll_interval() {
        let result = ConfigBuilder::new().poll_interval(0).build();
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::InvalidConfig {
                key: "POLL_INTERVAL_MINUTES",
                ..
            } => {}
            other => panic!("expected InvalidConfig, got {:?}", other),
        }
    }

    #[test]
    fn builder_rejects_zero_timeout() {
        let result = ConfigBuilder::new().request_timeout(0).build();
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::InvalidConfig {
                key: "REQUEST_TIMEOUT_SECS",
                ..
            } => {}
            other => panic!("expected InvalidConfig, got {:?}", other),
        }
    }

    #[test]
    fn builder_empty_providers_allowed() {
        let config = ConfigBuilder::new()
            .providers(vec![])
            .build()
            .unwrap();
        assert!(config.enabled_providers.is_empty());
    }

    #[test]
    fn builder_empty_notifiers_allowed() {
        let config = ConfigBuilder::new()
            .notifiers(vec![])
            .build()
            .unwrap();
        assert!(config.enabled_notifiers.is_empty());
    }

    #[test]
    fn builder_rejects_oversized_poll_interval() {
        let result = ConfigBuilder::new()
            .poll_interval(MAX_POLL_INTERVAL_MINUTES + 1)
            .build();
        match result.unwrap_err() {
            AppError::InvalidConfig {
                key: "POLL_INTERVAL_MINUTES",
                ..
            } => {}
            other => panic!("expected InvalidConfig, got {:?}", other),
        }
    }

    #[test]
    fn builder_accepts_max_poll_interval() {
        let config = ConfigBuilder::new()
            .poll_interval(MAX_POLL_INTERVAL_MINUTES)
            .build()
            .unwrap();
        assert_eq!(config.poll_interval_minutes, MAX_POLL_INTERVAL_MINUTES);
    }

    #[test]
    fn builder_rejects_oversized_timeout() {
        let result = ConfigBuilder::new()
            .request_timeout(MAX_REQUEST_TIMEOUT_SECS + 1)
            .build();
        match result.unwrap_err() {
            AppError::InvalidConfig {
                key: "REQUEST_TIMEOUT_SECS",
                ..
            } => {}
            other => panic!("expected InvalidConfig, got {:?}", other),
        }
    }

    #[test]
    fn builder_accepts_max_timeout() {
        let config = ConfigBuilder::new()
            .request_timeout(MAX_REQUEST_TIMEOUT_SECS)
            .build()
            .unwrap();
        assert_eq!(config.request_timeout_secs, MAX_REQUEST_TIMEOUT_SECS);
    }

    #[test]
    fn build_client_succeeds() {
        let config = ConfigBuilder::new().build().unwrap();
        assert!(config.build_client().is_ok());
    }

    #[test]
    fn provider_param_returns_env_value() {
        std::env::set_var("PROVIDER_TESTPROVIDER_FEED_URL", "https://example.com/feed");
        let result = provider_param("testprovider", "FEED_URL");
        std::env::remove_var("PROVIDER_TESTPROVIDER_FEED_URL");
        assert_eq!(result, Some("https://example.com/feed".to_string()));
    }

    #[test]
    fn provider_param_returns_none_when_unset() {
        std::env::remove_var("PROVIDER_ABSENT_PROVIDER_FEED_URL");
        assert_eq!(provider_param("absent_provider", "FEED_URL"), None);
    }

    #[test]
    fn provider_param_uppercases_name_and_key() {
        std::env::set_var("PROVIDER_MYPROVIDER_API_URL", "https://api.example.com");
        let result = provider_param("myprovider", "API_URL");
        std::env::remove_var("PROVIDER_MYPROVIDER_API_URL");
        assert_eq!(result, Some("https://api.example.com".to_string()));
    }

    #[test]
    fn notifier_param_returns_env_value() {
        std::env::set_var("NOTIFIER_TESTNOTIFIER_BOT_TOKEN", "secret-token");
        let result = notifier_param("testnotifier", "BOT_TOKEN");
        std::env::remove_var("NOTIFIER_TESTNOTIFIER_BOT_TOKEN");
        assert_eq!(result, Some("secret-token".to_string()));
    }

    #[test]
    fn notifier_param_returns_none_when_unset() {
        std::env::remove_var("NOTIFIER_ABSENT_NOTIFIER_CHAT_ID");
        assert_eq!(notifier_param("absent_notifier", "CHAT_ID"), None);
    }

    #[test]
    fn builder_accepts_failure_threshold() {
        let config = ConfigBuilder::new()
            .failure_threshold(Some(3))
            .build()
            .unwrap();
        assert_eq!(config.failure_threshold, Some(3));
    }

    #[test]
    fn builder_rejects_zero_failure_threshold() {
        let result = ConfigBuilder::new().failure_threshold(Some(0)).build();
        match result.unwrap_err() {
            AppError::InvalidConfig { key: "PROVIDER_FAILURE_THRESHOLD", .. } => {}
            other => panic!("expected InvalidConfig, got {:?}", other),
        }
    }

    #[test]
    fn builder_rejects_zero_backoff_max_cycles() {
        let result = ConfigBuilder::new().backoff_max_cycles(0).build();
        match result.unwrap_err() {
            AppError::InvalidConfig { key: "PROVIDER_BACKOFF_MAX_CYCLES", .. } => {}
            other => panic!("expected InvalidConfig, got {:?}", other),
        }
    }

    #[test]
    fn builder_accepts_custom_backoff_max_cycles() {
        let config = ConfigBuilder::new().backoff_max_cycles(16).build().unwrap();
        assert_eq!(config.backoff_max_cycles, 16);
    }
}
