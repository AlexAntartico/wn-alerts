use crate::config::Config;
use crate::core::incident::Incident;
use crate::core::provider::StatusProvider;
use crate::core::state::AppState;
use crate::error::AppError;
use crate::notifiers::Notifier;
use chrono::Utc;

pub struct Scheduler {
    config: Config,
    state: AppState,
    client: reqwest::Client,
    providers: Vec<Box<dyn StatusProvider>>,
    notifiers: Vec<Box<dyn Notifier>>,
}

impl Scheduler {
    pub fn new(config: Config) -> Result<Self, AppError> {
        let client = config.build_client()?;
        let state = crate::core::state::load_state(&config.state_file_path)?;

        let providers = crate::providers::build_all(&config)?;
        let notifiers = crate::notifiers::build_all(&config)?;

        if providers.is_empty() {
            return Err(AppError::NoProvidersEnabled);
        }

        if notifiers.is_empty() {
            return Err(AppError::NoNotifiersEnabled);
        }

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

        self.providers.iter().for_each(|provider| {
            tracing::info!(provider = provider.name(), "Checking status...");
        });

        let provider_count = self.providers.len();
        for i in 0..provider_count {
            self.process_provider_by_index(i, &now).await;
        }

        if let Err(e) = crate::core::state::save_state(&self.config.state_file_path, &self.state) {
            tracing::error!("Failed to save state: {}", e);
        }

        tracing::info!("--- Poll cycle complete ---");
    }

    async fn process_provider_by_index(&mut self, index: usize, now: &chrono::DateTime<Utc>) {
        let provider = &self.providers[index];
        let name = provider.name();

        match provider.check(&self.client).await {
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
}
