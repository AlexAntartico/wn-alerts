use crate::config::Config;
use crate::core::incident::Incident;
use crate::core::provider::StatusProvider;
use crate::core::state::AppState;
use crate::error::AppError;
use crate::notifiers::Notifier;
use chrono::Utc;
use std::sync::Arc;

/// Result of a single provider check, collected during fan-in phase.
struct ProviderCheckResult {
    name: &'static str,
    result: Result<Vec<Incident>, AppError>,
}

pub struct Scheduler {
    config: Config,
    state: AppState,
    client: reqwest::Client,
    providers: Vec<Arc<dyn StatusProvider>>,
    notifiers: Vec<Box<dyn Notifier>>,
}

impl Scheduler {
    pub fn new(config: Config) -> Result<Self, AppError> {
        let client = config.build_client()?;
        let state = crate::core::state::load_state(&config.state_file_path)?;

        let boxed_providers = crate::providers::build_all(&config)?;
        let notifiers = crate::notifiers::build_all(&config)?;

        if boxed_providers.is_empty() {
            return Err(AppError::NoProvidersEnabled);
        }

        if notifiers.is_empty() {
            return Err(AppError::NoNotifiersEnabled);
        }

        // Convert Box<dyn StatusProvider> to Arc<dyn StatusProvider> for concurrent polling
        let providers: Vec<Arc<dyn StatusProvider>> = boxed_providers
            .into_iter()
            .map(|b| Arc::from(b) as Arc<dyn StatusProvider>)
            .collect();

        Ok(Self {
            config,
            state,
            client,
            providers,
            notifiers,
        })
    }

    pub async fn run(&mut self) -> Result<(), AppError> {
        let poll_duration = std::time::Duration::from_secs(self.config.poll_interval_minutes * 60);

        self.print_startup_summary();

        loop {
            self.tick().await;

            tokio::select! {
                _ = tokio::time::sleep(poll_duration) => {},
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("Received SIGINT, saving state and shutting down...");
                    crate::core::state::save_state(&self.config.state_file_path, &self.state).ok();
                    tracing::info!("Shutdown complete");
                    return Ok(());
                }
            }
        }
    }

    async fn tick(&mut self) {
        let now = Utc::now();
        tracing::info!("--- Poll cycle start ---");

        // Phase 1: Fan-out — poll all providers concurrently
        let results = self.fetch_all_providers_concurrently().await;

        // Phase 2: Fan-in — process results sequentially (state mutation is fast)
        for result in results {
            self.process_provider_result(result, &now).await;
        }

        if let Err(e) = crate::core::state::save_state(&self.config.state_file_path, &self.state) {
            tracing::error!("Failed to save state: {}", e);
        }

        tracing::info!("--- Poll cycle complete ---");
    }

    /// Fan-out: spawn concurrent HTTP requests to all providers.
    /// Returns collected results sorted alphabetically by provider name.
    async fn fetch_all_providers_concurrently(&self) -> Vec<ProviderCheckResult> {
        let mut handles: Vec<(&'static str, tokio::task::JoinHandle<ProviderCheckResult>)> =
            Vec::with_capacity(self.providers.len());

        for provider in &self.providers {
            let provider = Arc::clone(provider);
            let client = self.client.clone();
            let name = provider.name(); // capture before spawn

            let handle = tokio::spawn(async move {
                tracing::info!(provider = name, "Checking status...");
                let result = provider.check(&client).await;
                ProviderCheckResult { name, result }
            });

            handles.push((name, handle));
        }

        // Collect results from all spawned tasks
        let mut results = Vec::with_capacity(handles.len());
        for (name, handle) in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => {
                    // Task panicked or was cancelled — log with provider identity
                    tracing::error!(
                        provider = name,
                        error = %e,
                        "Provider task panicked or was cancelled",
                    );
                }
            }
        }

        // Sort by provider name for deterministic ordering
        results.sort_by_key(|r| r.name);
        results
    }

    /// Fan-in: process a single provider's result, updating state and sending notifications.
    async fn process_provider_result(&mut self, check: ProviderCheckResult, now: &chrono::DateTime<Utc>) {
        let name = check.name;

        match check.result {
            Ok(incidents) => {
                tracing::info!(
                    provider = name,
                    total = incidents.len(),
                    "Fetched {} incident(s)",
                    incidents.len(),
                );

                if incidents.is_empty() {
                    tracing::info!(
                        provider = name,
                        "No active incidents — all services operating normally"
                    );
                }

                let new_incidents: Vec<_> = incidents
                    .iter()
                    .filter(|incident| !self.state.has_seen(name, &incident.id))
                    .cloned()
                    .collect();

                for incident in &new_incidents {
                    tracing::info!(
                        provider = name,
                        id = %incident.id,
                        title = %incident.title,
                        "New incident detected",
                    );
                    self.notify_all(incident).await;
                    self.state.mark_seen(name, incident.id.clone());
                }

                for incident in incidents
                    .iter()
                    .filter(|i| !new_incidents.iter().any(|n| n.id == i.id))
                {
                    tracing::debug!(
                        provider = name,
                        id = %incident.id,
                        "Skipping already-seen incident: {}",
                        incident.title,
                    );
                }

                self.state.set_poll_time(name, now.to_rfc3339());
            }
            Err(e) => {
                tracing::error!(
                    provider = name,
                    error = %e,
                    "Provider check failed",
                );
                // Still record the poll time — "we tried at this time" is useful state
                self.state.set_poll_time(name, now.to_rfc3339());
            }
        }
    }

    async fn notify_all(&self, incident: &Incident) {
        for notifier in &self.notifiers {
            let name = notifier.name();
            match notifier.notify(&self.client, incident).await {
                Ok(_) => {
                    tracing::info!(
                        notifier = name,
                        provider = %incident.provider,
                        "Notification sent",
                    );
                }
                Err(e) => {
                    tracing::error!(
                        notifier = name,
                        error = %e,
                        "Notification failed",
                    );
                }
            }
        }
    }

    fn print_startup_summary(&self) {
        let provider_names: Vec<&str> = self.providers.iter().map(|p| p.name()).collect();
        let notifier_names: Vec<&str> = self.notifiers.iter().map(|n| n.name()).collect();

        tracing::info!("wn-alerts started");
        tracing::info!(
            poll_interval_minutes = self.config.poll_interval_minutes,
            providers = %provider_names.join(", "),
            notifiers = %notifier_names.join(", "),
            "Polling every {} minutes",
            self.config.poll_interval_minutes,
        );
    }
}

#[cfg(test)]
impl Scheduler {
    /// Create a scheduler for testing with pre-built providers.
    /// Uses a noop notifier and bypasses the normal provider building.
    pub fn new_for_test(config: Config, providers: Vec<Box<dyn StatusProvider>>) -> Self {
        use async_trait::async_trait;

        struct NoopNotifier;

        #[async_trait]
        impl crate::notifiers::Notifier for NoopNotifier {
            fn name(&self) -> &'static str {
                "noop"
            }
            async fn notify(
                &self,
                _client: &reqwest::Client,
                _incident: &Incident,
            ) -> Result<(), AppError> {
                Ok(())
            }
        }

        let client = config.build_client().unwrap();
        let state = crate::core::state::load_state(&config.state_file_path).unwrap();

        let arc_providers: Vec<Arc<dyn StatusProvider>> = providers
            .into_iter()
            .map(|b| Arc::from(b) as Arc<dyn StatusProvider>)
            .collect();

        Scheduler {
            config,
            state,
            client,
            providers: arc_providers,
            notifiers: vec![Box::new(NoopNotifier)],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigBuilder;

    fn state_path(dir: &tempfile::TempDir) -> String {
        dir.path().join("state.json").to_str().unwrap().to_string()
    }

    fn assert_err_kind<T>(result: Result<T, AppError>, expected: &str, f: impl Fn(&AppError) -> bool) {
        match result {
            Err(ref e) if f(e) => {}
            Err(e) => panic!("expected {}, got {:?}", expected, e),
            Ok(_) => panic!("expected Err({}), got Ok", expected),
        }
    }

    #[test]
    fn rejects_empty_providers() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = ConfigBuilder::new()
            .providers(vec![])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .build()
            .unwrap();

        assert_err_kind(
            Scheduler::new(config),
            "NoProvidersEnabled",
            |e| matches!(e, AppError::NoProvidersEnabled),
        );
    }

    #[test]
    fn rejects_empty_notifiers() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = ConfigBuilder::new()
            .providers(vec!["azure".into()])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .build()
            .unwrap();

        assert_err_kind(
            Scheduler::new(config),
            "NoNotifiersEnabled",
            |e| matches!(e, AppError::NoNotifiersEnabled),
        );
    }

    #[test]
    fn rejects_unknown_provider() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = ConfigBuilder::new()
            .providers(vec!["does-not-exist".into()])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .build()
            .unwrap();

        assert_err_kind(
            Scheduler::new(config),
            "UnknownProvider",
            |e| matches!(e, AppError::UnknownProvider(_)),
        );
    }

    #[test]
    fn rejects_unknown_notifier() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = ConfigBuilder::new()
            .providers(vec!["azure".into()])
            .notifiers(vec!["does-not-exist".into()])
            .state_file(state_path(&tmp))
            .build()
            .unwrap();

        assert_err_kind(
            Scheduler::new(config),
            "UnknownNotifier",
            |e| matches!(e, AppError::UnknownNotifier(_)),
        );
    }

    #[test]
    fn providers_wrapped_in_arc() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = ConfigBuilder::new()
            .providers(vec!["azure".into(), "aws".into()])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .build()
            .unwrap();

        // Build providers with default URLs (they won't be called, just checked for structure)
        let providers = crate::providers::build_all(&config).unwrap();
        let scheduler = Scheduler::new_for_test(config, providers);

        // Verify providers are stored as Arc (can be cloned and shared)
        assert_eq!(scheduler.providers.len(), 2);
        assert_eq!(scheduler.providers[0].name(), "azure");
        assert_eq!(scheduler.providers[1].name(), "aws");

        // Verify Arc reference counting works (clone should succeed)
        let _arc_clone = Arc::clone(&scheduler.providers[0]);
    }

    #[tokio::test]
    async fn fetch_all_providers_concurrently_returns_all_results() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let tmp = tempfile::TempDir::new().unwrap();
        let mock_server = MockServer::start().await;

        let empty_rss = r#"<?xml version="1.0"?><rss version="2.0"><channel><title>T</title></channel></rss>"#;

        Mock::given(method("GET"))
            .and(path("/feed.rss"))
            .respond_with(ResponseTemplate::new(200).set_body_string(empty_rss))
            .mount(&mock_server)
            .await;

        let feed_url = format!("{}/feed.rss", mock_server.uri());

        // Use new_unvalidated to skip domain validation for mock server
        let providers: Vec<Box<dyn StatusProvider>> = vec![
            Box::new(crate::providers::azure::new_unvalidated(feed_url.clone())),
            Box::new(crate::providers::aws::new_unvalidated(feed_url)),
        ];

        let config = ConfigBuilder::new()
            .providers(vec![])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .build()
            .unwrap();

        let scheduler = Scheduler::new_for_test(config, providers);
        let results = scheduler.fetch_all_providers_concurrently().await;

        // Should get results from both providers
        assert_eq!(results.len(), 2);

        // Both should succeed (empty feeds)
        assert!(results.iter().all(|r| r.result.is_ok()));

        // Results are sorted alphabetically by provider name
        assert_eq!(results[0].name, "aws");
        assert_eq!(results[1].name, "azure");
    }

    #[tokio::test]
    async fn fetch_all_providers_concurrently_handles_provider_errors() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let tmp = tempfile::TempDir::new().unwrap();
        let mock_server = MockServer::start().await;

        // Return 500 error to trigger provider check failure
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock_server)
            .await;

        let feed_url = mock_server.uri();

        let providers: Vec<Box<dyn StatusProvider>> = vec![
            Box::new(crate::providers::azure::new_unvalidated(feed_url)),
        ];

        let config = ConfigBuilder::new()
            .providers(vec![])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .build()
            .unwrap();

        let scheduler = Scheduler::new_for_test(config, providers);
        let results = scheduler.fetch_all_providers_concurrently().await;

        // Should get result even on error
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "azure");
        assert!(results[0].result.is_err());
    }

    #[tokio::test]
    async fn fetch_all_providers_concurrently_is_actually_concurrent() {
        use std::time::Instant;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let tmp = tempfile::TempDir::new().unwrap();
        let server_a = MockServer::start().await;
        let server_b = MockServer::start().await;

        let empty_rss = r#"<?xml version="1.0"?><rss version="2.0"><channel><title>T</title></channel></rss>"#;

        // Each mock responds after 200ms delay
        Mock::given(method("GET"))
            .and(path("/slow.rss"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(empty_rss)
                    .set_delay(std::time::Duration::from_millis(200)),
            )
            .mount(&server_a)
            .await;

        Mock::given(method("GET"))
            .and(path("/slow.rss"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(empty_rss)
                    .set_delay(std::time::Duration::from_millis(200)),
            )
            .mount(&server_b)
            .await;

        let feed_url_a = format!("{}/slow.rss", server_a.uri());
        let feed_url_b = format!("{}/slow.rss", server_b.uri());

        let providers: Vec<Box<dyn StatusProvider>> = vec![
            Box::new(crate::providers::azure::new_unvalidated(feed_url_a)),
            Box::new(crate::providers::aws::new_unvalidated(feed_url_b)),
        ];

        let config = ConfigBuilder::new()
            .providers(vec![])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .build()
            .unwrap();

        let scheduler = Scheduler::new_for_test(config, providers);

        let start = Instant::now();
        let results = scheduler.fetch_all_providers_concurrently().await;
        let elapsed = start.elapsed();

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.result.is_ok()));

        // Concurrent: ~200ms. Sequential: ~400ms.
        // Use 350ms as threshold to account for overhead while still proving concurrency.
        assert!(
            elapsed < std::time::Duration::from_millis(350),
            "polling was sequential (took {:?}, expected <350ms for 2x200ms concurrent)",
            elapsed,
        );
    }

    #[tokio::test]
    async fn process_provider_result_sets_poll_time_on_error() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let tmp = tempfile::TempDir::new().unwrap();
        let mock_server = MockServer::start().await;

        // Return 500 error to trigger provider check failure
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock_server)
            .await;

        let feed_url = mock_server.uri();

        let providers: Vec<Box<dyn StatusProvider>> = vec![
            Box::new(crate::providers::azure::new_unvalidated(feed_url)),
        ];

        let config = ConfigBuilder::new()
            .providers(vec![])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .build()
            .unwrap();

        let mut scheduler = Scheduler::new_for_test(config, providers);
        let results = scheduler.fetch_all_providers_concurrently().await;

        // Process the error result
        let now = Utc::now();
        for result in results {
            scheduler.process_provider_result(result, &now).await;
        }

        // Verify set_poll_time was called even on error
        let provider_state = scheduler.state.providers.get("azure");
        assert!(
            provider_state.is_some(),
            "provider state should exist after error"
        );
        assert!(
            provider_state.unwrap().last_poll.is_some(),
            "last_poll should be set even on error"
        );
    }

    #[tokio::test]
    async fn fetch_all_providers_concurrently_returns_deterministic_order() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let tmp = tempfile::TempDir::new().unwrap();
        let server_fast = MockServer::start().await;
        let server_slow = MockServer::start().await;

        let empty_rss = r#"<?xml version="1.0"?><rss version="2.0"><channel><title>T</title></channel></rss>"#;

        // "twilio" (alphabetically later) responds fast
        Mock::given(method("GET"))
            .and(path("/fast.rss"))
            .respond_with(ResponseTemplate::new(200).set_body_string(empty_rss))
            .mount(&server_fast)
            .await;

        // "azure" (alphabetically earlier) responds slow
        Mock::given(method("GET"))
            .and(path("/slow.rss"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(empty_rss)
                    .set_delay(std::time::Duration::from_millis(100)),
            )
            .mount(&server_slow)
            .await;

        // Register providers in reverse alphabetical order
        let providers: Vec<Box<dyn StatusProvider>> = vec![
            Box::new(crate::providers::twilio::new_unvalidated(format!(
                "{}/fast.rss",
                server_fast.uri()
            ))),
            Box::new(crate::providers::azure::new_unvalidated(format!(
                "{}/slow.rss",
                server_slow.uri()
            ))),
        ];

        let config = ConfigBuilder::new()
            .providers(vec![])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .build()
            .unwrap();

        let scheduler = Scheduler::new_for_test(config, providers);
        let results = scheduler.fetch_all_providers_concurrently().await;

        // Results should be sorted alphabetically, regardless of completion order
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "azure");
        assert_eq!(results[1].name, "twilio");
    }
}
