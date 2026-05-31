use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Incident {
    pub provider: String,
    pub id: String,
    pub title: String,
    pub description: String,
    pub link: String,
    pub occurred_at: String,
}

impl Incident {
    /// Stable content fingerprint identifying the *current state* of an incident.
    ///
    /// Status pages (statuspage.io and the like) reuse the same `guid`/`id` for
    /// an incident's entire lifetime, mutating the title/description/timestamp in
    /// place as it moves Investigating → Identified → Monitoring → Resolved.
    /// Deduping on `id` alone therefore delivers only the first sighting and
    /// silently drops every later update. Pairing `id` with this fingerprint lets
    /// the scheduler re-notify whenever the visible content actually changes.
    ///
    /// Uses FNV-1a rather than `std`'s `DefaultHasher` (whose algorithm is
    /// explicitly unspecified and may change between toolchains): this value is
    /// persisted to `state.json` and compared across restarts, so it must be
    /// deterministic forever.
    pub fn fingerprint(&self) -> String {
        const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

        // Folds `bytes` into `hash`, then mixes a separator so that differing
        // field boundaries (e.g. ("ab","c") vs ("a","bc")) cannot collide.
        fn mix(mut hash: u64, bytes: &[u8]) -> u64 {
            for &b in bytes {
                hash ^= b as u64;
                hash = hash.wrapping_mul(FNV_PRIME);
            }
            hash ^= 0xff;
            hash.wrapping_mul(FNV_PRIME)
        }

        let mut hash = FNV_OFFSET;
        hash = mix(hash, self.title.as_bytes());
        hash = mix(hash, self.description.as_bytes());
        hash = mix(hash, self.occurred_at.as_bytes());

        format!("{hash:016x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn incident(title: &str, description: &str, occurred_at: &str) -> Incident {
        Incident {
            provider: "test".into(),
            id: "same-id".into(),
            title: title.into(),
            description: description.into(),
            link: "https://example.com".into(),
            occurred_at: occurred_at.into(),
        }
    }

    #[test]
    fn fingerprint_is_stable_for_identical_content() {
        let a = incident("Investigating", "Looking into it", "Sun, 31 May 2026 06:00:00 GMT");
        let b = incident("Investigating", "Looking into it", "Sun, 31 May 2026 06:00:00 GMT");
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn fingerprint_changes_when_description_changes() {
        let investigating = incident("TLS issue", "Investigating - ...", "Sun, 31 May 2026 03:16:00 GMT");
        let identified = incident("TLS issue", "Identified - a fix is being implemented", "Sun, 31 May 2026 06:41:00 GMT");
        assert_ne!(
            investigating.fingerprint(),
            identified.fingerprint(),
            "a new status update must produce a different fingerprint"
        );
    }

    #[test]
    fn fingerprint_ignores_non_content_fields() {
        // provider/id/link are identity, not content — they don't affect the fingerprint.
        let mut a = incident("Title", "Body", "Sun, 31 May 2026 06:00:00 GMT");
        let mut b = a.clone();
        b.provider = "different".into();
        b.id = "different".into();
        b.link = "https://other.example.com".into();
        a.provider = "x".into();
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn fingerprint_respects_field_boundaries() {
        // Moving content across the field boundary must not collide.
        let split_a = incident("ab", "c", "");
        let split_b = incident("a", "bc", "");
        assert_ne!(split_a.fingerprint(), split_b.fingerprint());
    }
}
