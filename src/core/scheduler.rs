use crate::config::Config;
use crate::core::incident::Incident;
use crate::core::provider::StatusProvider;
use crate::core::state::AppState;
use crate::error::AppError;
use crate::notifiers::Notifier;
use chrono::Utc;
use std::sync::Arc;

/// Resolves when SIGINT (Ctrl-C) or SIGTERM is received, returning the signal name.
/// On non-Unix platforms only SIGINT is handled.
async fn shutdown_signal() -> &'static str {
    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate(),
        )
        .expect("failed to register SIGTERM handler");

        tokio::select! {
            _ = tokio::signal::ctrl_c() => "SIGINT",
            _ = sigterm.recv() => "SIGTERM",
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.ok();
        "SIGINT"
    }
}

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
    notifiers: Vec<Arc<dyn Notifier>>,
}

impl Scheduler {
    pub fn new(config: Config) -> Result<Self, AppError> {
        let client = config.build_client()?;
        let state = crate::core::state::load_state(&config.state_file_path)?;

        let boxed_providers = crate::providers::build_all(&config)?;
        let boxed_notifiers = crate::notifiers::build_all(&config)?;

        if boxed_providers.is_empty() {
            return Err(AppError::NoProvidersEnabled);
        }

        if boxed_notifiers.is_empty() {
            return Err(AppError::NoNotifiersEnabled);
        }

        let providers: Vec<Arc<dyn StatusProvider>> = boxed_providers
            .into_iter()
            .map(|b| Arc::from(b) as Arc<dyn StatusProvider>)
            .collect();

        let notifiers: Vec<Arc<dyn Notifier>> = boxed_notifiers
            .into_iter()
            .map(|b| Arc::from(b) as Arc<dyn Notifier>)
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
        self.run_with_shutdown(shutdown_signal()).await
    }

    async fn run_with_shutdown<F>(&mut self, shutdown: F) -> Result<(), AppError>
    where
        F: std::future::Future<Output = &'static str>,
    {
        let poll_duration = std::time::Duration::from_secs(self.config.poll_interval_minutes * 60);

        self.print_startup_summary();

        tokio::pin!(shutdown);

        loop {
            self.tick().await;

            tokio::select! {
                _ = tokio::time::sleep(poll_duration) => {},
                signal = &mut shutdown => {
                    tracing::info!("Received {signal}, saving state and shutting down...");
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

        // Phase 1: Partition — skip backed-off providers, collect the rest for polling
        let mut to_poll: Vec<Arc<dyn StatusProvider>> = Vec::new();
        for provider in &self.providers {
            let name = provider.name();
            let remaining = self.state.backoff_cycles_remaining(name);
            if remaining > 0 {
                tracing::warn!(
                    provider = name,
                    cycles_remaining = remaining,
                    "Provider is backing off — skipping this cycle",
                );
                self.state.decrement_backoff(name);
            } else {
                to_poll.push(Arc::clone(provider));
            }
        }

        // Phase 2: Fan-out — poll active providers concurrently
        let results = self.fetch_all_providers_concurrently(&to_poll).await;

        // Phase 3: Fan-in — process results sequentially (state mutation is fast)
        for result in results {
            self.process_provider_result(result, &now).await;
        }

        if let Err(e) = crate::core::state::save_state(&self.config.state_file_path, &self.state) {
            tracing::error!("Failed to save state: {}", e);
        }

        tracing::info!("--- Poll cycle complete ---");
    }

    /// Fan-out: spawn concurrent HTTP requests for the given providers.
    /// Returns collected results sorted alphabetically by provider name.
    async fn fetch_all_providers_concurrently(
        &self,
        providers: &[Arc<dyn StatusProvider>],
    ) -> Vec<ProviderCheckResult> {
        let mut handles: Vec<(&'static str, tokio::task::JoinHandle<ProviderCheckResult>)> =
            Vec::with_capacity(providers.len());

        for provider in providers {
            let provider = Arc::clone(provider);
            let client = self.client.clone();
            let name = provider.name();

            let handle = tokio::spawn(async move {
                tracing::info!(provider = name, "Checking status...");
                let result = provider.check(&client).await;
                ProviderCheckResult { name, result }
            });

            handles.push((name, handle));
        }

        let mut results = Vec::with_capacity(handles.len());
        for (name, handle) in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => {
                    tracing::error!(
                        provider = name,
                        error = %e,
                        "Provider task panicked or was cancelled",
                    );
                }
            }
        }

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
                    let notified = self.notify_all(incident).await;
                    if notified {
                        self.state.mark_seen(name, incident.id.clone());
                    } else {
                        tracing::warn!(
                            provider = name,
                            id = %incident.id,
                            "All notifiers failed — incident will retry next cycle",
                        );
                    }
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

                if self.config.failure_threshold.is_some() {
                    self.state.record_success(name);
                }
                self.state.set_poll_time(name, now.to_rfc3339());
            }
            Err(e) => {
                tracing::error!(
                    provider = name,
                    error = %e,
                    "Provider check failed",
                );
                self.state.set_poll_time(name, now.to_rfc3339());

                if let Some(threshold) = self.config.failure_threshold {
                    self.state.record_failure(name);
                    let failures = self.state.consecutive_failures(name);
                    if failures >= threshold {
                        // 2^failures, capped — checked_shl guards against u32 overflow
                        let cycles = 1u32
                            .checked_shl(failures)
                            .unwrap_or(u32::MAX)
                            .min(self.config.backoff_max_cycles);
                        tracing::warn!(
                            provider = name,
                            consecutive_failures = failures,
                            backoff_cycles = cycles,
                            "Provider hit failure threshold — backing off for {} cycle(s)",
                            cycles,
                        );
                        self.state.set_backoff(name, cycles);
                    }
                }
            }
        }
    }

    async fn notify_all(&self, incident: &Incident) -> bool {
        let mut set = tokio::task::JoinSet::new();

        for notifier in &self.notifiers {
            let notifier = Arc::clone(notifier);
            let client = self.client.clone();
            let incident = incident.clone();
            set.spawn(async move {
                let name = notifier.name();
                match notifier.notify(&client, &incident).await {
                    Ok(_) => {
                        tracing::info!(
                            notifier = name,
                            provider = %incident.provider,
                            "Notification sent",
                        );
                        true
                    }
                    Err(e) => {
                        tracing::error!(
                            notifier = name,
                            error = %e,
                            "Notification failed",
                        );
                        false
                    }
                }
            });
        }

        let mut any_succeeded = false;
        while let Some(outcome) = set.join_next().await {
            match outcome {
                Ok(succeeded) => any_succeeded |= succeeded,
                Err(e) => {
                    tracing::error!(error = %e, "Notifier task panicked or was cancelled");
                }
            }
        }
        any_succeeded
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
            notifiers: vec![Arc::new(NoopNotifier) as Arc<dyn crate::notifiers::Notifier>],
        }
    }

    /// Create a scheduler with explicit notifiers for testing notification failure paths.
    pub fn new_for_test_with_notifiers(
        config: Config,
        providers: Vec<Box<dyn StatusProvider>>,
        notifiers: Vec<Box<dyn crate::notifiers::Notifier>>,
    ) -> Self {
        let client = config.build_client().unwrap();
        let state = crate::core::state::load_state(&config.state_file_path).unwrap();

        let arc_providers: Vec<Arc<dyn StatusProvider>> = providers
            .into_iter()
            .map(|b| Arc::from(b) as Arc<dyn StatusProvider>)
            .collect();

        let arc_notifiers: Vec<Arc<dyn crate::notifiers::Notifier>> = notifiers
            .into_iter()
            .map(|b| Arc::from(b) as Arc<dyn crate::notifiers::Notifier>)
            .collect();

        Scheduler {
            config,
            state,
            client,
            providers: arc_providers,
            notifiers: arc_notifiers,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use crate::config::ConfigBuilder;

    fn state_path(dir: &tempfile::TempDir) -> String {
        dir.path().join("state.json").to_str().unwrap().to_string()
    }

    fn make_incident(id: &str) -> Incident {
        Incident {
            provider: "stub".into(),
            id: id.into(),
            title: "Test Incident".into(),
            description: "Test description".into(),
            link: "https://example.com".into(),
            occurred_at: "Thu, 21 May 2026 12:00:00 GMT".into(),
        }
    }

    struct SucceedingNotifier;

    #[async_trait]
    impl crate::notifiers::Notifier for SucceedingNotifier {
        fn name(&self) -> &'static str { "succeeding" }
        async fn notify(&self, _: &reqwest::Client, _: &Incident) -> Result<(), AppError> {
            Ok(())
        }
    }

    struct FailingNotifier;

    #[async_trait]
    impl crate::notifiers::Notifier for FailingNotifier {
        fn name(&self) -> &'static str { "failing" }
        async fn notify(&self, _: &reqwest::Client, _: &Incident) -> Result<(), AppError> {
            Err(AppError::TelegramApi { status: 500, body: "server error".into() })
        }
    }

    struct PanicNotifier;

    #[async_trait]
    impl crate::notifiers::Notifier for PanicNotifier {
        fn name(&self) -> &'static str { "panicking" }
        async fn notify(&self, _: &reqwest::Client, _: &Incident) -> Result<(), AppError> {
            panic!("simulated notifier panic");
        }
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
        let providers = scheduler.providers.clone();
        let results = scheduler.fetch_all_providers_concurrently(&providers).await;

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
        let providers = scheduler.providers.clone();
        let results = scheduler.fetch_all_providers_concurrently(&providers).await;

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
        let providers = scheduler.providers.clone();
        let results = scheduler.fetch_all_providers_concurrently(&providers).await;
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
        let providers = scheduler.providers.clone();
        let results = scheduler.fetch_all_providers_concurrently(&providers).await;

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
        let providers = scheduler.providers.clone();
        let results = scheduler.fetch_all_providers_concurrently(&providers).await;

        // Results should be sorted alphabetically, regardless of completion order
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "azure");
        assert_eq!(results[1].name, "twilio");
    }

    #[tokio::test]
    async fn saves_state_on_shutdown_signal() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state_file = state_path(&tmp);
        let config = ConfigBuilder::new()
            .providers(vec![])
            .notifiers(vec![])
            .state_file(state_file.clone())
            .build()
            .unwrap();

        let mut scheduler = Scheduler::new_for_test_with_notifiers(
            config,
            vec![],
            vec![Box::new(SucceedingNotifier)],
        );

        // Immediately-resolving future simulates receiving a signal without OS involvement
        scheduler
            .run_with_shutdown(std::future::ready("SIGTERM"))
            .await
            .unwrap();

        assert!(
            std::path::Path::new(&state_file).exists(),
            "state file should be written to disk on shutdown"
        );
    }

    #[tokio::test]
    async fn run_with_shutdown_returns_signal_name() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = ConfigBuilder::new()
            .providers(vec![])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .build()
            .unwrap();

        let mut scheduler = Scheduler::new_for_test_with_notifiers(
            config,
            vec![],
            vec![Box::new(SucceedingNotifier)],
        );

        // run_with_shutdown accepts any &'static str from the shutdown future,
        // including "SIGINT" — verify it completes without error for both names.
        let result = scheduler
            .run_with_shutdown(std::future::ready("SIGINT"))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn notify_all_returns_true_when_notifier_succeeds() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = ConfigBuilder::new()
            .providers(vec![])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .build()
            .unwrap();
        let scheduler = Scheduler::new_for_test_with_notifiers(
            config,
            vec![],
            vec![Box::new(SucceedingNotifier)],
        );
        assert!(scheduler.notify_all(&make_incident("i1")).await);
    }

    #[tokio::test]
    async fn notify_all_returns_false_when_all_notifiers_fail() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = ConfigBuilder::new()
            .providers(vec![])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .build()
            .unwrap();
        let scheduler = Scheduler::new_for_test_with_notifiers(
            config,
            vec![],
            vec![Box::new(FailingNotifier)],
        );
        assert!(!scheduler.notify_all(&make_incident("i1")).await);
    }

    #[tokio::test]
    async fn notify_all_returns_true_when_at_least_one_notifier_succeeds() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = ConfigBuilder::new()
            .providers(vec![])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .build()
            .unwrap();
        let scheduler = Scheduler::new_for_test_with_notifiers(
            config,
            vec![],
            vec![Box::new(FailingNotifier), Box::new(SucceedingNotifier)],
        );
        assert!(scheduler.notify_all(&make_incident("i1")).await);
    }

    #[tokio::test]
    async fn incident_marked_seen_when_notification_succeeds() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = ConfigBuilder::new()
            .providers(vec![])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .build()
            .unwrap();
        let mut scheduler = Scheduler::new_for_test_with_notifiers(
            config,
            vec![],
            vec![Box::new(SucceedingNotifier)],
        );

        let result = ProviderCheckResult {
            name: "stub",
            result: Ok(vec![make_incident("i1")]),
        };
        scheduler.process_provider_result(result, &Utc::now()).await;

        assert!(
            scheduler.state.has_seen("stub", "i1"),
            "incident should be marked seen after successful notification"
        );
    }

    #[tokio::test]
    async fn incident_not_marked_seen_when_all_notifications_fail() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = ConfigBuilder::new()
            .providers(vec![])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .build()
            .unwrap();
        let mut scheduler = Scheduler::new_for_test_with_notifiers(
            config,
            vec![],
            vec![Box::new(FailingNotifier)],
        );

        let result = ProviderCheckResult {
            name: "stub",
            result: Ok(vec![make_incident("i1")]),
        };
        scheduler.process_provider_result(result, &Utc::now()).await;

        assert!(
            !scheduler.state.has_seen("stub", "i1"),
            "incident should not be marked seen when all notifiers failed"
        );
    }

    #[tokio::test]
    async fn notify_all_runs_notifiers_concurrently() {
        use std::time::{Duration, Instant};

        struct SlowNotifier {
            delay: Duration,
        }

        #[async_trait]
        impl crate::notifiers::Notifier for SlowNotifier {
            fn name(&self) -> &'static str {
                "slow"
            }
            async fn notify(
                &self,
                _client: &reqwest::Client,
                _incident: &Incident,
            ) -> Result<(), AppError> {
                tokio::time::sleep(self.delay).await;
                Ok(())
            }
        }

        let tmp = tempfile::TempDir::new().unwrap();
        let config = ConfigBuilder::new()
            .providers(vec![])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .build()
            .unwrap();

        // Two notifiers each sleeping 200ms — concurrent: ~200ms, sequential: ~400ms
        let scheduler = Scheduler::new_for_test_with_notifiers(
            config,
            vec![],
            vec![
                Box::new(SlowNotifier { delay: Duration::from_millis(200) }),
                Box::new(SlowNotifier { delay: Duration::from_millis(200) }),
            ],
        );

        let start = Instant::now();
        let succeeded = scheduler.notify_all(&make_incident("i1")).await;
        let elapsed = start.elapsed();

        assert!(succeeded, "both notifiers should succeed");
        assert!(
            elapsed < Duration::from_millis(350),
            "notifiers ran sequentially (took {:?}, expected <350ms for 2x200ms concurrent)",
            elapsed,
        );
    }

    #[tokio::test]
    async fn notify_all_isolates_panicking_notifier() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = ConfigBuilder::new()
            .providers(vec![])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .build()
            .unwrap();

        let scheduler = Scheduler::new_for_test_with_notifiers(
            config,
            vec![],
            vec![Box::new(PanicNotifier)],
        );

        // Panic inside the spawned task must not propagate — notify_all returns false
        let succeeded = scheduler.notify_all(&make_incident("i1")).await;
        assert!(!succeeded, "panicking notifier should count as failed, not propagate");
    }

    #[tokio::test]
    async fn notify_all_succeeds_when_one_notifier_panics_and_another_succeeds() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = ConfigBuilder::new()
            .providers(vec![])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .build()
            .unwrap();

        let scheduler = Scheduler::new_for_test_with_notifiers(
            config,
            vec![],
            vec![Box::new(PanicNotifier), Box::new(SucceedingNotifier)],
        );

        // The healthy notifier still fires — panic in one task must not abort the others
        let succeeded = scheduler.notify_all(&make_incident("i1")).await;
        assert!(succeeded, "healthy notifier should still succeed despite sibling panic");
    }

    // ── Backoff behaviour ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn provider_enters_backoff_after_threshold_failures() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let tmp = tempfile::TempDir::new().unwrap();
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock_server)
            .await;

        let config = ConfigBuilder::new()
            .providers(vec![])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .failure_threshold(Some(3))
            .backoff_max_cycles(32)
            .build()
            .unwrap();

        let providers: Vec<Box<dyn StatusProvider>> = vec![
            Box::new(crate::providers::azure::new_unvalidated(mock_server.uri())),
        ];
        let mut scheduler = Scheduler::new_for_test(config, providers);

        // Fail 3 times to hit the threshold
        for _ in 0..3 {
            let providers = scheduler.providers.clone();
            let results = scheduler.fetch_all_providers_concurrently(&providers).await;
            let now = Utc::now();
            for result in results {
                scheduler.process_provider_result(result, &now).await;
            }
        }

        // After 3 failures backoff_cycles_remaining should be 2^3 = 8
        assert_eq!(
            scheduler.state.backoff_cycles_remaining("azure"),
            8,
            "expected 8 backoff cycles (2^3) after 3 failures"
        );
    }

    #[tokio::test]
    async fn backoff_cycles_decrement_and_provider_skipped() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = ConfigBuilder::new()
            .providers(vec![])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .failure_threshold(Some(1))
            .backoff_max_cycles(4)
            .build()
            .unwrap();

        // Pre-seed state: azure is already in backoff with 3 cycles remaining
        let mut scheduler = Scheduler::new_for_test(config, vec![
            Box::new(crate::providers::azure::new_unvalidated("http://unused".into())),
        ]);
        scheduler.state.set_backoff("azure", 3);

        // tick() should skip azure and decrement to 2
        scheduler.run_with_shutdown(std::future::ready("SIGTERM")).await.unwrap();

        assert_eq!(
            scheduler.state.backoff_cycles_remaining("azure"),
            2,
            "backoff should decrement by 1 each skipped cycle"
        );
    }

    #[tokio::test]
    async fn successful_poll_resets_backoff_state() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let tmp = tempfile::TempDir::new().unwrap();
        let mock_server = MockServer::start().await;
        let empty_rss = r#"<?xml version="1.0"?><rss version="2.0"><channel><title>T</title></channel></rss>"#;

        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(empty_rss))
            .mount(&mock_server)
            .await;

        let config = ConfigBuilder::new()
            .providers(vec![])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .failure_threshold(Some(2))
            .backoff_max_cycles(8)
            .build()
            .unwrap();

        let providers: Vec<Box<dyn StatusProvider>> = vec![
            Box::new(crate::providers::azure::new_unvalidated(mock_server.uri())),
        ];
        let mut scheduler = Scheduler::new_for_test(config, providers);

        // Pre-seed failure state
        scheduler.state.record_failure("azure");
        scheduler.state.record_failure("azure");
        scheduler.state.set_backoff("azure", 4);
        // Manually clear backoff so the provider gets polled this cycle
        scheduler.state.set_backoff("azure", 0);

        let providers = scheduler.providers.clone();
        let results = scheduler.fetch_all_providers_concurrently(&providers).await;
        let now = Utc::now();
        for result in results {
            scheduler.process_provider_result(result, &now).await;
        }

        assert_eq!(scheduler.state.consecutive_failures("azure"), 0, "success should reset failure counter");
        assert_eq!(scheduler.state.backoff_cycles_remaining("azure"), 0, "success should clear backoff");
    }

    #[tokio::test]
    async fn backoff_capped_at_max_cycles() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let tmp = tempfile::TempDir::new().unwrap();
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock_server)
            .await;

        let config = ConfigBuilder::new()
            .providers(vec![])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .failure_threshold(Some(1))
            .backoff_max_cycles(4)
            .build()
            .unwrap();

        let providers: Vec<Box<dyn StatusProvider>> = vec![
            Box::new(crate::providers::azure::new_unvalidated(mock_server.uri())),
        ];
        let mut scheduler = Scheduler::new_for_test(config, providers);

        // Run enough failures that 2^N would exceed the cap
        for _ in 0..6 {
            scheduler.state.set_backoff("azure", 0); // force a poll each time
            let providers = scheduler.providers.clone();
            let results = scheduler.fetch_all_providers_concurrently(&providers).await;
            let now = Utc::now();
            for result in results {
                scheduler.process_provider_result(result, &now).await;
            }
        }

        assert!(
            scheduler.state.backoff_cycles_remaining("azure") <= 4,
            "backoff must not exceed backoff_max_cycles=4, got {}",
            scheduler.state.backoff_cycles_remaining("azure")
        );
    }

    #[tokio::test]
    async fn no_backoff_when_threshold_not_configured() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let tmp = tempfile::TempDir::new().unwrap();
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock_server)
            .await;

        // Default config — no failure_threshold set
        let config = ConfigBuilder::new()
            .providers(vec![])
            .notifiers(vec![])
            .state_file(state_path(&tmp))
            .build()
            .unwrap();

        let providers: Vec<Box<dyn StatusProvider>> = vec![
            Box::new(crate::providers::azure::new_unvalidated(mock_server.uri())),
        ];
        let mut scheduler = Scheduler::new_for_test(config, providers);

        // Fail many times
        for _ in 0..5 {
            let providers = scheduler.providers.clone();
            let results = scheduler.fetch_all_providers_concurrently(&providers).await;
            let now = Utc::now();
            for result in results {
                scheduler.process_provider_result(result, &now).await;
            }
        }

        assert_eq!(
            scheduler.state.backoff_cycles_remaining("azure"),
            0,
            "backoff must not activate when failure_threshold is not configured"
        );
    }
}
