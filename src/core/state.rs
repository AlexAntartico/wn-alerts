use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

use crate::error::AppError;

pub const MAX_SEEN_IDS_PER_PROVIDER: usize = 10_000;

/// A set bounded to `MAX_SEEN_IDS_PER_PROVIDER` entries.
/// When the cap is reached the oldest inserted entry is evicted (FIFO).
/// Serializes as a plain JSON array for backward compatibility.
#[derive(Debug, Clone, Default)]
pub struct BoundedSeenSet {
    ids: HashSet<String>,
    order: VecDeque<String>,
}

impl BoundedSeenSet {
    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    pub fn insert(&mut self, id: String) {
        if self.ids.contains(&id) {
            return;
        }
        if self.ids.len() >= MAX_SEEN_IDS_PER_PROVIDER {
            if let Some(oldest) = self.order.pop_front() {
                self.ids.remove(&oldest);
            }
        }
        self.ids.insert(id.clone());
        self.order.push_back(id);
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

impl Serialize for BoundedSeenSet {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(self.ids.len()))?;
        for id in &self.ids {
            seq.serialize_element(id)?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for BoundedSeenSet {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let ids: Vec<String> = Vec::deserialize(deserializer)?;
        let mut set = BoundedSeenSet::default();
        for id in ids {
            set.insert(id);
        }
        Ok(set)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppState {
    pub providers: HashMap<String, ProviderState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderState {
    pub seen_ids: BoundedSeenSet,
    pub last_poll: Option<String>,
    /// Consecutive poll failures since last success. Resets to 0 on any successful poll.
    #[serde(default)]
    pub consecutive_failures: u32,
    /// Cycles remaining to skip before polling this provider again.
    #[serde(default)]
    pub backoff_cycles_remaining: u32,
}

impl AppState {
    pub fn has_seen(&self, provider: &str, id: &str) -> bool {
        self.providers
            .get(provider)
            .map(|s| s.seen_ids.contains(id))
            .unwrap_or(false)
    }

    pub fn mark_seen(&mut self, provider: &str, id: String) {
        self.providers
            .entry(provider.to_string())
            .or_default()
            .seen_ids
            .insert(id);
    }

    pub fn set_poll_time(&mut self, provider: &str, time: String) {
        self.providers
            .entry(provider.to_string())
            .or_default()
            .last_poll = Some(time);
    }

    pub fn record_failure(&mut self, provider: &str) {
        self.providers
            .entry(provider.to_string())
            .or_default()
            .consecutive_failures += 1;
    }

    /// Reset failure streak and clear any remaining backoff on a successful poll.
    pub fn record_success(&mut self, provider: &str) {
        let entry = self.providers.entry(provider.to_string()).or_default();
        entry.consecutive_failures = 0;
        entry.backoff_cycles_remaining = 0;
    }

    pub fn set_backoff(&mut self, provider: &str, cycles: u32) {
        self.providers
            .entry(provider.to_string())
            .or_default()
            .backoff_cycles_remaining = cycles;
    }

    pub fn decrement_backoff(&mut self, provider: &str) {
        if let Some(s) = self.providers.get_mut(provider) {
            s.backoff_cycles_remaining = s.backoff_cycles_remaining.saturating_sub(1);
        }
    }

    pub fn consecutive_failures(&self, provider: &str) -> u32 {
        self.providers
            .get(provider)
            .map(|s| s.consecutive_failures)
            .unwrap_or(0)
    }

    pub fn backoff_cycles_remaining(&self, provider: &str) -> u32 {
        self.providers
            .get(provider)
            .map(|s| s.backoff_cycles_remaining)
            .unwrap_or(0)
    }
}

pub fn load_state(path: &str) -> Result<AppState, AppError> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(AppState::default()),
        Err(e) => return Err(AppError::Io(e)),
    };
    serde_json::from_str(&content).map_err(AppError::Json)
}

pub fn save_state(path: &str, state: &AppState) -> Result<(), AppError> {
    let content = serde_json::to_string_pretty(state)?;
    let tmp = format!("{}.tmp", path);
    write_private(&tmp, content.as_bytes())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(unix)]
fn write_private(path: &str, content: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .write_all(content)
}

#[cfg(not(unix))]
fn write_private(path: &str, content: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, content)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── BoundedSeenSet ────────────────────────────────────────────────────────

    #[test]
    fn bounded_set_contains_inserted_ids() {
        let mut set = BoundedSeenSet::default();
        set.insert("a".into());
        set.insert("b".into());
        assert!(set.contains("a"));
        assert!(set.contains("b"));
        assert!(!set.contains("c"));
    }

    #[test]
    fn bounded_set_ignores_duplicates() {
        let mut set = BoundedSeenSet::default();
        set.insert("id-1".into());
        set.insert("id-1".into());
        set.insert("id-1".into());
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn bounded_set_caps_at_max_size() {
        let mut set = BoundedSeenSet::default();
        for i in 0..MAX_SEEN_IDS_PER_PROVIDER + 50 {
            set.insert(format!("id-{}", i));
        }
        assert_eq!(set.len(), MAX_SEEN_IDS_PER_PROVIDER);
    }

    #[test]
    fn bounded_set_evicts_oldest_on_overflow() {
        let mut set = BoundedSeenSet::default();
        // Insert exactly MAX entries, then one more.
        for i in 0..=MAX_SEEN_IDS_PER_PROVIDER {
            set.insert(format!("id-{:010}", i)); // zero-padded so names are predictable
        }
        // The first inserted entry should have been evicted.
        assert!(!set.contains("id-0000000000"), "oldest entry should be evicted");
        assert!(
            set.contains(&format!("id-{:010}", MAX_SEEN_IDS_PER_PROVIDER)),
            "newest entry should be present"
        );
        assert_eq!(set.len(), MAX_SEEN_IDS_PER_PROVIDER);
    }

    #[test]
    fn bounded_set_serde_roundtrip() {
        let mut set = BoundedSeenSet::default();
        set.insert("alpha".into());
        set.insert("beta".into());
        set.insert("gamma".into());

        let json = serde_json::to_string(&set).unwrap();
        let restored: BoundedSeenSet = serde_json::from_str(&json).unwrap();

        assert!(restored.contains("alpha"));
        assert!(restored.contains("beta"));
        assert!(restored.contains("gamma"));
        assert_eq!(restored.len(), 3);
    }

    #[test]
    fn bounded_set_prunes_on_deserialize_if_over_limit() {
        // Build a JSON array with MAX+1 entries.
        let ids: Vec<String> = (0..=MAX_SEEN_IDS_PER_PROVIDER)
            .map(|i| format!("id-{}", i))
            .collect();
        let json = serde_json::to_string(&ids).unwrap();

        let set: BoundedSeenSet = serde_json::from_str(&json).unwrap();
        assert_eq!(set.len(), MAX_SEEN_IDS_PER_PROVIDER);
    }

    // ── AppState ──────────────────────────────────────────────────────────────

    #[test]
    fn default_state_is_empty() {
        let state = AppState::default();
        assert!(state.providers.is_empty());
    }

    #[test]
    fn tracks_seen_incidents_per_provider() {
        let mut state = AppState::default();
        assert!(!state.has_seen("azure", "abc"));

        state.mark_seen("azure", "abc".into());
        assert!(state.has_seen("azure", "abc"));
        assert!(!state.has_seen("azure", "def"));
        assert!(!state.has_seen("aws", "abc"));
    }

    #[test]
    fn tracks_poll_times_per_provider() {
        let mut state = AppState::default();
        state.set_poll_time("azure", "2026-05-21T20:00:00Z".into());
        state.set_poll_time("aws", "2026-05-21T20:01:00Z".into());

        let azure = state.providers.get("azure").unwrap();
        assert_eq!(azure.last_poll.as_deref(), Some("2026-05-21T20:00:00Z"));

        let aws = state.providers.get("aws").unwrap();
        assert_eq!(aws.last_poll.as_deref(), Some("2026-05-21T20:01:00Z"));
    }

    #[test]
    fn serialization_roundtrip() {
        let mut state = AppState::default();
        state.mark_seen("azure", "guid-1".into());
        state.mark_seen("azure", "guid-2".into());
        state.set_poll_time("azure", "2026-05-21T20:00:00Z".into());

        let json = serde_json::to_string(&state).unwrap();
        let restored: AppState = serde_json::from_str(&json).unwrap();

        assert!(restored.has_seen("azure", "guid-1"));
        assert!(restored.has_seen("azure", "guid-2"));
        assert_eq!(
            restored.providers.get("azure").and_then(|s| s.last_poll.as_deref()),
            Some("2026-05-21T20:00:00Z")
        );
    }

    #[test]
    fn load_missing_file_returns_default() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent.json");
        let state = load_state(path.to_str().unwrap()).unwrap();
        assert!(state.providers.is_empty());
    }

    #[test]
    fn load_corrupt_file_returns_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("state.json");
        std::fs::write(&path, b"not valid json").unwrap();
        assert!(load_state(path.to_str().unwrap()).is_err());
    }

    #[test]
    fn save_and_load_roundtrip_via_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("state.json");
        let path_str = path.to_str().unwrap();

        let mut state = AppState::default();
        state.mark_seen("azure", "guid-1".into());
        state.mark_seen("aws", "guid-2".into());
        state.set_poll_time("azure", "2026-05-21T20:00:00Z".into());

        save_state(path_str, &state).unwrap();

        let loaded = load_state(path_str).unwrap();
        assert!(loaded.has_seen("azure", "guid-1"));
        assert!(!loaded.has_seen("azure", "guid-2"));
        assert!(loaded.has_seen("aws", "guid-2"));
        assert_eq!(
            loaded.providers.get("azure").and_then(|s| s.last_poll.as_deref()),
            Some("2026-05-21T20:00:00Z")
        );
    }

    #[test]
    fn save_leaves_no_tmp_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("state.json");
        let path_str = path.to_str().unwrap();

        save_state(path_str, &AppState::default()).unwrap();

        assert!(path.exists(), "state file should exist");
        assert!(
            !tmp.path().join("state.json.tmp").exists(),
            "tmp file should be cleaned up after rename"
        );
    }

    #[cfg(unix)]
    #[test]
    fn save_state_creates_file_with_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("state.json");
        let path_str = path.to_str().unwrap();

        save_state(path_str, &AppState::default()).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "state file should be mode 0o600, got {:#o}", mode & 0o777);
    }

    // ── Backoff tracking ──────────────────────────────────────────────────────

    #[test]
    fn record_failure_increments_counter() {
        let mut state = AppState::default();
        state.record_failure("azure");
        state.record_failure("azure");
        assert_eq!(state.consecutive_failures("azure"), 2);
        assert_eq!(state.consecutive_failures("aws"), 0);
    }

    #[test]
    fn record_success_resets_failure_counter_and_backoff() {
        let mut state = AppState::default();
        state.record_failure("azure");
        state.record_failure("azure");
        state.set_backoff("azure", 8);
        state.record_success("azure");
        assert_eq!(state.consecutive_failures("azure"), 0);
        assert_eq!(state.backoff_cycles_remaining("azure"), 0);
    }

    #[test]
    fn set_and_decrement_backoff() {
        let mut state = AppState::default();
        state.set_backoff("azure", 3);
        assert_eq!(state.backoff_cycles_remaining("azure"), 3);
        state.decrement_backoff("azure");
        assert_eq!(state.backoff_cycles_remaining("azure"), 2);
        state.decrement_backoff("azure");
        state.decrement_backoff("azure");
        assert_eq!(state.backoff_cycles_remaining("azure"), 0);
        // saturating — must not underflow
        state.decrement_backoff("azure");
        assert_eq!(state.backoff_cycles_remaining("azure"), 0);
    }

    #[test]
    fn backoff_fields_persist_via_serde() {
        let mut state = AppState::default();
        state.record_failure("azure");
        state.record_failure("azure");
        state.set_backoff("azure", 4);

        let json = serde_json::to_string(&state).unwrap();
        let restored: AppState = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.consecutive_failures("azure"), 2);
        assert_eq!(restored.backoff_cycles_remaining("azure"), 4);
    }

    #[test]
    fn backoff_fields_default_to_zero_on_legacy_state() {
        // Simulate a state.json written before backoff fields existed
        let legacy = r#"{"providers":{"azure":{"seen_ids":[],"last_poll":null}}}"#;
        let state: AppState = serde_json::from_str(legacy).unwrap();
        assert_eq!(state.consecutive_failures("azure"), 0);
        assert_eq!(state.backoff_cycles_remaining("azure"), 0);
    }

    #[test]
    fn save_overwrites_existing_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("state.json");
        let path_str = path.to_str().unwrap();

        let mut state = AppState::default();
        state.mark_seen("azure", "old-guid".into());
        save_state(path_str, &state).unwrap();

        let mut state2 = AppState::default();
        state2.mark_seen("azure", "new-guid".into());
        save_state(path_str, &state2).unwrap();

        let loaded = load_state(path_str).unwrap();
        assert!(loaded.has_seen("azure", "new-guid"));
        assert!(!loaded.has_seen("azure", "old-guid"));
    }
}
