use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

use crate::error::AppError;

pub const MAX_SEEN_IDS_PER_PROVIDER: usize = 10_000;

/// A map of incident id → content fingerprint, bounded to
/// `MAX_SEEN_IDS_PER_PROVIDER` entries. When the cap is reached the oldest
/// inserted id is evicted (FIFO); updating the fingerprint of an existing id
/// leaves its eviction position unchanged.
///
/// The fingerprint lets us tell a brand-new incident apart from an in-place
/// status update to one we've already alerted on (see [`Incident::fingerprint`]).
///
/// Serializes as a JSON object `{ id: fingerprint }`. For backward
/// compatibility it also deserializes the legacy form — a plain JSON array of
/// ids — assigning each a sentinel empty fingerprint, so the first poll after
/// upgrading re-syncs every incident to its real content state.
///
/// [`Incident::fingerprint`]: crate::core::incident::Incident::fingerprint
#[derive(Debug, Clone, Default)]
pub struct BoundedSeenSet {
    fingerprints: HashMap<String, String>,
    order: VecDeque<String>,
}

impl BoundedSeenSet {
    pub fn contains(&self, id: &str) -> bool {
        self.fingerprints.contains_key(id)
    }

    /// The fingerprint recorded for `id`, or `None` if the id has never been seen.
    pub fn fingerprint(&self, id: &str) -> Option<&str> {
        self.fingerprints.get(id).map(String::as_str)
    }

    pub fn insert(&mut self, id: String, fingerprint: String) {
        if let Some(existing) = self.fingerprints.get_mut(&id) {
            // Already tracked — just refresh the fingerprint, keep FIFO position.
            *existing = fingerprint;
            return;
        }
        if self.fingerprints.len() >= MAX_SEEN_IDS_PER_PROVIDER {
            if let Some(oldest) = self.order.pop_front() {
                self.fingerprints.remove(&oldest);
            }
        }
        self.fingerprints.insert(id.clone(), fingerprint);
        self.order.push_back(id);
    }

    pub fn len(&self) -> usize {
        self.fingerprints.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fingerprints.is_empty()
    }
}

impl Serialize for BoundedSeenSet {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.fingerprints.len()))?;
        for (id, fingerprint) in &self.fingerprints {
            map.serialize_entry(id, fingerprint)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for BoundedSeenSet {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Accept both the current map form and the legacy plain-array form.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Legacy(Vec<String>),
            Versioned(HashMap<String, String>),
        }

        let mut set = BoundedSeenSet::default();
        match Repr::deserialize(deserializer)? {
            Repr::Legacy(ids) => {
                for id in ids {
                    set.insert(id, String::new());
                }
            }
            Repr::Versioned(map) => {
                for (id, fingerprint) in map {
                    set.insert(id, fingerprint);
                }
            }
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

    /// The fingerprint last recorded for an incident, or `None` if unseen.
    /// Compare against [`Incident::fingerprint`] to decide whether the incident
    /// is new, unchanged, or carries a fresh status update.
    ///
    /// [`Incident::fingerprint`]: crate::core::incident::Incident::fingerprint
    pub fn seen_fingerprint(&self, provider: &str, id: &str) -> Option<&str> {
        self.providers
            .get(provider)
            .and_then(|s| s.seen_ids.fingerprint(id))
    }

    pub fn mark_seen(&mut self, provider: &str, id: String, fingerprint: String) {
        self.providers
            .entry(provider.to_string())
            .or_default()
            .seen_ids
            .insert(id, fingerprint);
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
        set.insert("a".into(), "fp-a".into());
        set.insert("b".into(), "fp-b".into());
        assert!(set.contains("a"));
        assert!(set.contains("b"));
        assert!(!set.contains("c"));
        assert_eq!(set.fingerprint("a"), Some("fp-a"));
        assert_eq!(set.fingerprint("c"), None);
    }

    #[test]
    fn bounded_set_ignores_duplicate_ids() {
        let mut set = BoundedSeenSet::default();
        set.insert("id-1".into(), "fp".into());
        set.insert("id-1".into(), "fp".into());
        set.insert("id-1".into(), "fp".into());
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn bounded_set_updates_fingerprint_in_place() {
        let mut set = BoundedSeenSet::default();
        set.insert("id-1".into(), "old".into());
        set.insert("id-1".into(), "new".into());
        assert_eq!(set.len(), 1, "re-inserting an id must not grow the set");
        assert_eq!(set.fingerprint("id-1"), Some("new"), "fingerprint should be updated");
    }

    #[test]
    fn bounded_set_caps_at_max_size() {
        let mut set = BoundedSeenSet::default();
        for i in 0..MAX_SEEN_IDS_PER_PROVIDER + 50 {
            set.insert(format!("id-{}", i), "fp".into());
        }
        assert_eq!(set.len(), MAX_SEEN_IDS_PER_PROVIDER);
    }

    #[test]
    fn bounded_set_evicts_oldest_on_overflow() {
        let mut set = BoundedSeenSet::default();
        // Insert exactly MAX entries, then one more.
        for i in 0..=MAX_SEEN_IDS_PER_PROVIDER {
            set.insert(format!("id-{:010}", i), "fp".into()); // zero-padded so names are predictable
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
    fn bounded_set_updating_fingerprint_does_not_reset_eviction_order() {
        let mut set = BoundedSeenSet::default();
        // Fill to capacity.
        for i in 0..MAX_SEEN_IDS_PER_PROVIDER {
            set.insert(format!("id-{:010}", i), "fp".into());
        }
        // Refresh the oldest entry's fingerprint — must NOT move it to the back.
        set.insert("id-0000000000".into(), "refreshed".into());
        // Inserting a new id should still evict the (still-oldest) id-0.
        set.insert("id-new".into(), "fp".into());
        assert!(!set.contains("id-0000000000"), "refreshing fingerprint must not save an entry from FIFO eviction");
        assert!(set.contains("id-new"));
    }

    #[test]
    fn bounded_set_serde_roundtrip() {
        let mut set = BoundedSeenSet::default();
        set.insert("alpha".into(), "fp-alpha".into());
        set.insert("beta".into(), "fp-beta".into());
        set.insert("gamma".into(), "fp-gamma".into());

        let json = serde_json::to_string(&set).unwrap();
        let restored: BoundedSeenSet = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.fingerprint("alpha"), Some("fp-alpha"));
        assert_eq!(restored.fingerprint("beta"), Some("fp-beta"));
        assert_eq!(restored.fingerprint("gamma"), Some("fp-gamma"));
        assert_eq!(restored.len(), 3);
    }

    #[test]
    fn bounded_set_deserializes_legacy_array_form() {
        // Pre-fingerprint state files stored seen ids as a plain JSON array.
        let json = r#"["legacy-1","legacy-2"]"#;
        let set: BoundedSeenSet = serde_json::from_str(json).unwrap();
        assert!(set.contains("legacy-1"));
        assert!(set.contains("legacy-2"));
        // Legacy entries carry a sentinel empty fingerprint so the next poll re-syncs them.
        assert_eq!(set.fingerprint("legacy-1"), Some(""));
    }

    #[test]
    fn bounded_set_prunes_on_deserialize_if_over_limit() {
        // Build a legacy JSON array with MAX+1 entries.
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

        state.mark_seen("azure", "abc".into(), "fp-1".into());
        assert!(state.has_seen("azure", "abc"));
        assert_eq!(state.seen_fingerprint("azure", "abc"), Some("fp-1"));
        assert!(!state.has_seen("azure", "def"));
        assert!(!state.has_seen("aws", "abc"));
    }

    #[test]
    fn mark_seen_updates_fingerprint_for_existing_incident() {
        let mut state = AppState::default();
        state.mark_seen("cloudflare", "inc-1".into(), "investigating".into());
        state.mark_seen("cloudflare", "inc-1".into(), "identified".into());
        assert_eq!(state.seen_fingerprint("cloudflare", "inc-1"), Some("identified"));
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
        state.mark_seen("azure", "guid-1".into(), "fp-1".into());
        state.mark_seen("azure", "guid-2".into(), "fp-2".into());
        state.set_poll_time("azure", "2026-05-21T20:00:00Z".into());

        let json = serde_json::to_string(&state).unwrap();
        let restored: AppState = serde_json::from_str(&json).unwrap();

        assert!(restored.has_seen("azure", "guid-1"));
        assert!(restored.has_seen("azure", "guid-2"));
        assert_eq!(restored.seen_fingerprint("azure", "guid-1"), Some("fp-1"));
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
        state.mark_seen("azure", "guid-1".into(), "fp-1".into());
        state.mark_seen("aws", "guid-2".into(), "fp-2".into());
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
        state.mark_seen("azure", "old-guid".into(), "fp-old".into());
        save_state(path_str, &state).unwrap();

        let mut state2 = AppState::default();
        state2.mark_seen("azure", "new-guid".into(), "fp-new".into());
        save_state(path_str, &state2).unwrap();

        let loaded = load_state(path_str).unwrap();
        assert!(loaded.has_seen("azure", "new-guid"));
        assert!(!loaded.has_seen("azure", "old-guid"));
    }
}
