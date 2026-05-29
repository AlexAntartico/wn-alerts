# Security Audit: wn-alerts

## Audit Date: 2026-05-26

## Summary

Multi-provider service status monitor with pluggable notifiers. Supports Azure, AWS, Twilio, Airship, Cloudflare, GitHub, and Imperva status feeds. Sends alerts via Telegram.

**Dependency scan:** `cargo audit` — 0 advisories across 226 crates
**Lints:** `cargo clippy --all-targets` — 0 warnings
**Tests:** 101 pass (77 unit + 24 integration), 0 failures

---

## Findings

### Finding 1 — SSRF bypass via HTTP redirect following

| | |
|---|---|
| **Severity** | Medium |
| **Status** | **Fixed** |
| **Location** | `src/providers/rss_common.rs` (line ~120), `src/config.rs` (client builder) |

**Description:** `reqwest` followed up to 10 HTTP redirects by default. The SSRF validation in `RssProvider::validate_feed_url()` only checked the *initial* URL — domain allowlist, scheme, and private-IP blocking were applied before the request was sent. If an allowed domain (e.g. `status.example.com`) returned a 3xx redirect to an internal IP (`http://169.254.169.254/latest/meta-data/`), reqwest would follow it and the response body would be parsed as RSS.

**Attack scenario:** A compromised status page on an allowed domain redirects the feed request to the AWS EC2 metadata endpoint or an internal corporate service. The response is parsed (likely failing RSS parsing, but the TCP connection was made and the response body was loaded into memory).

**Exploitability:** Low-medium. Requires either a compromised status page or DNS hijacking of an allowed domain. AWS IMDSv2 requires a PUT with a token header, so simple GET redirects to IMDS won't leak credentials. However, other internal services may not have such protections.

**Fix applied:** Defense-in-depth approach:
1. Added `.redirect(reqwest::redirect::Policy::none())` to the reqwest client builder in `src/config.rs:135` — reqwest no longer follows redirects at the HTTP level
2. Added explicit 3xx check in `src/providers/rss_common.rs:127` — returns `AppError::InvalidConfig` with clear "redirects are not followed" message before any body is read
3. Added integration test `rss_provider_rejects_redirect_response` — validates the full defense chain with a mock 301 → IMDS attack scenario

Redirects are now blocked at two layers: the HTTP client refuses to follow them, and the provider code explicitly rejects 3xx responses with a descriptive error log.

---

### Finding 2 — Unbounded response body (provider feeds)

| | |
|---|---|
| **Severity** | Low |
| **Status** | Open |
| **Location** | `src/providers/rss_common.rs:123` |

**Description:** `response.bytes().await?` reads the entire HTTP response body into memory with no size limit. A compromised or malicious provider feed could serve a multi-gigabyte response, causing the daemon to exhaust available memory (OOM).

**Impact:** Denial of service — the process is killed by the OOM killer, stopping all monitoring.

**Recommended fix:** Check `Content-Length` header before reading, and/or enforce a maximum body size:

```rust
const MAX_FEED_SIZE: usize = 10 * 1024 * 1024; // 10 MB

let response = client.get(&self.feed_url).send().await?.error_for_status()?;
if response.content_length().unwrap_or(0) as usize > MAX_FEED_SIZE {
    return Err(AppError::InvalidConfig {
        key: self.config_key,
        value: format!("response too large: {} bytes", response.content_length().unwrap_or(0)),
    });
}
let bytes = response.bytes().await?;
if bytes.len() > MAX_FEED_SIZE {
    return Err(/* ... */);
}
```

---

### Finding 3 — Unbounded response body (Telegram API errors)

| | |
|---|---|
| **Severity** | Low |
| **Status** | Open |
| **Location** | `src/notifiers/telegram.rs:88` |

**Description:** When the Telegram API returns a non-2xx status, `response.text().await.unwrap_or_default()` reads the full error body with no size limit. Same OOM risk as Finding 2, though less likely since the Telegram API is a well-known endpoint.

**Recommended fix:** Limit the error body read to a reasonable size (e.g. 4 KB is more than enough for an API error message).

---

## Existing Security Controls (all verified intact)

| Control | Status | Location | Notes |
|---------|--------|----------|-------|
| SSRF domain allowlist | ✅ | `src/providers/rss_common.rs` | Validates host against per-provider allowed domains |
| Private IP blocking | ✅ | `src/providers/rss_common.rs` | Blocks loopback, private, link-local, unspecified, broadcast (v4+v6) |
| Scheme validation (http/https only) | ✅ | `src/providers/rss_common.rs` | Rejects `file://`, `ftp://`, etc. |
| HTTP redirect blocking | ✅ | `src/config.rs:135`, `src/providers/rss_common.rs:127` | Client-level `Policy::none()` + explicit 3xx rejection |
| Config upper bounds | ✅ | `src/config.rs` | Poll interval max 1440 min, timeout max 300 sec |
| Non-panicking HTTP client | ✅ | `src/config.rs` | Returns `AppError::ClientInit` on failure |
| State file permissions (0o600) | ✅ | `src/core/state.rs` | Unix-only mode set; Windows fallback uses default |
| Atomic state file writes | ✅ | `src/core/state.rs` | Write to `.tmp`, then `rename()` — no partial writes |
| State ID cap (10,000/provider) | ✅ | `src/core/state.rs` | FIFO eviction via `BoundedSeenSet` — prevents unbounded growth |
| Sensitive data redaction in Debug | ✅ | `src/config.rs`, `src/notifiers/telegram.rs` | Bot token, chat ID shown as `[REDACTED]` |
| Token leakage prevention | ✅ | `src/notifiers/telegram.rs` | `e.without_url()` strips URL from reqwest errors |
| User-Agent header | ✅ | `src/config.rs` | Set to `wn-alerts/0.1.0` |
| Rate limiting (Telegram) | ✅ | `src/notifiers/telegram.rs` | 1 second delay after each message |
| HTML escaping (Telegram) | ✅ | `src/utils/html.rs`, `src/notifiers/telegram.rs` | Decode-then-escape pattern; only `&` and `<` escaped (sufficient for Telegram HTML) |
| XML parsing safety | ✅ | `quick-xml 0.39.4` via `rss 2.0.13` | No external entity processing, no XXE vector |
| `.env` / `state.json` gitignored | ✅ | `.gitignore` | Not tracked in git; `.env.example` tracked (safe) |
| Graceful shutdown | ✅ | `src/core/scheduler.rs` | SIGINT saves state before exit |
| TLS verification | ✅ | reqwest default | Certificate verification enabled by default |

---

## Previous Security Decisions

### Token in URL path (not Authorization header)
- **Status:** Correct, confirmed
- **Reason:** Telegram Bot API requires the token in the URL path (`/bot<TOKEN>/sendMessage`). An Authorization header returns 404. Token leakage through error messages is mitigated by `e.without_url()`.

---

## Architecture

- **Providers (active):** Azure, AWS, Twilio, Airship, Cloudflare, GitHub, Imperva — all RSS via shared `RssProvider`
- **Providers (disabled):** Okta (auth issues), F5 (domain defunct) — code present, not compiled into binary
- **Notifiers:** Telegram (HTML-formatted, rate-limited)
- **Core:** Scheduler (sequential poll loop), State (atomic writes, bounded sets), Provider/Notifier traits
- **Utils:** HTML entity decode/escape, RFC 2822 date formatting

---

## Dependency Versions (key crates)

| Crate | Version | Notes |
|-------|---------|-------|
| `reqwest` | 0.12.28 | HTTP client; follows redirects by default |
| `rss` | 2.0.13 | RSS/Atom parsing |
| `quick-xml` | 0.39.4 | Underlying XML parser; safe by default |
| `url` | 2.5.8 | URL parsing for SSRF validation |
| `tokio` | 1.x | Async runtime |
| `serde` / `serde_json` | 1.x | Serialization |
| `chrono` | 0.4.x | Date parsing/formatting |

---

## Recommendations Priority

| # | Finding | Severity | Status |
|---|---------|----------|--------|
| ~~1~~ | ~~Add redirect policy to block SSRF bypass~~ | ~~Medium~~ | ✅ **Fixed** |
| 2 | Cap provider feed response body size | Low | Open |
| 3 | Cap Telegram error response body size | Low | Open |
