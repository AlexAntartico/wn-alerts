# Security Audit: wn-alerts

## Audit Date: 2026-05-22

## Summary

Multi-provider service status monitor with pluggable notifiers. Supports Azure, AWS, Twilio, Airship, Cloudflare, and GitHub status feeds. Sends alerts via Telegram.

## Security Decisions Record

### Item: Move token to Authorization header
- **Status:** Rejected
- **Reason:** The Telegram Bot API requires the token in the URL path — an Authorization header returns 404. We already fixed the opposite bug in the first code review. The real concern (token leaking through reqwest::Error log messages) was addressed instead with `e.without_url()`.

## Implemented Security Controls

| Control | Status | Location |
|---------|--------|----------|
| SSRF URL validation with domain allowlist | ✅ Implemented | `src/providers/rss_common.rs` |
| Private IP blocking | ✅ Implemented | `src/providers/rss_common.rs` |
| Scheme validation (http/https only) | ✅ Implemented | `src/providers/rss_common.rs` |
| Config upper bounds (poll_interval max 1440, timeout max 300) | ✅ Implemented | `src/config.rs` |
| Non-panicking HTTP client construction | ✅ Implemented | `src/config.rs` |
| File permissions (0o600) on state file | ✅ Implemented | `src/core/state.rs` |
| State pruning mechanism | ✅ Implemented | `src/core/state.rs` |
| Sensitive data redaction in Debug impls | ✅ Implemented | `src/config.rs`, `src/notifiers/telegram.rs` |
| User-Agent header on HTTP requests | ✅ Implemented | `src/config.rs` |
| Rate limiting on Telegram notifier | ✅ Implemented | `src/notifiers/telegram.rs` |
| Token leakage prevention via `without_url()` | ✅ Implemented | Error handling |

## Architecture

- **Providers:** Azure, AWS, Twilio, Airship, Cloudflare, GitHub (RSS + JSON APIs)
- **Notifiers:** Telegram
- **Core:** Scheduler, State (atomic writes), Provider trait
- **Utils:** HTML escaping, date formatting