# wn-alerts

Multi-provider service status monitor with pluggable notifiers. Polls status endpoints for Azure, AWS, Twilio, Airship, GitHub, Cloudflare, Imperva (and more) and sends alerts via Telegram when incidents are detected.

## Quick start

```bash
cp .env.example .env
# Edit .env with your Telegram bot token and chat ID
cargo run --release
```

## How it works

The daemon runs a poll loop every N minutes (default 5). Each tick:

1. **Fan-out phase**: All enabled providers are queried **concurrently** via `tokio::spawn` — a slow or unreachable provider doesn't block others. Wall-clock tick time is `max(provider_times)` instead of `sum(provider_times)`. For 7 providers at 30s timeout each, that's **30 seconds max** instead of 3.5 minutes.
2. **Fan-in phase**: Results are processed sequentially — new incidents (not previously seen) are routed to every enabled notifier, and marked seen in state
3. Each incident is marked seen immediately after notification, regardless of notification success — failed notifications are logged but do not block the cycle
4. Seen incident IDs are persisted to `state.json` after every cycle so alerts only fire once per incident

SIGINT (Ctrl+C) triggers a graceful shutdown with state save.

> **Note on `state.json` growth:** seen IDs are capped at 10,000 entries per provider (FIFO eviction). In practice this is several years of incidents and the file stays small. If you redeploy with a fresh `state.json` (or delete it) you will receive alerts for any incidents currently active in the feeds. To reset quietly, clear the file after a period of no active incidents.

## Architecture

```
src/
├── core/                       # Framework primitives
│   ├── provider.rs             # StatusProvider trait (name + async check)
│   ├── incident.rs             # Normalized Incident model
│   ├── state.rs                # Per-provider seen-ID tracking (JSON persistence)
│   └── scheduler.rs            # Poll loop — concurrent fan-out/fan-in orchestration
│
├── providers/                  # One module per service
│   ├── mod.rs                  # Factory — maps config names → concrete providers
│   ├── rss_common.rs           # Shared RssProvider struct (SSRF validation, size cap, RSS parsing)
│   ├── azure.rs                # Azure RSS feed provider
│   ├── aws.rs                  # AWS RSS feed provider
│   ├── twilio.rs               # Twilio RSS feed provider
│   ├── airship.rs              # Airship RSS feed provider
│   ├── github.rs               # GitHub RSS feed provider (statuspage.io)
│   ├── cloudflare.rs           # Cloudflare RSS feed provider (statuspage.io)
│   └── imperva.rs              # Imperva RSS feed provider (statuspage.io)
│
├── notifiers/                  # Notification channels
│   ├── mod.rs                  # Notifier trait + factory
│   └── telegram.rs             # Telegram notifier (HTML-formatted, rate-limited)
│
├── utils/
│   └── html.rs                 # HTML tag stripping, entity decode/escape, date formatting
│
├── config.rs                   # ConfigBuilder + env parsing + validation + HTTP client
├── error.rs                    # AppError enum (thiserror)
├── lib.rs                      # Crate root, re-exports
└── main.rs                     # Thin binary entry point
```

### Key traits

```rust
#[async_trait]
pub trait StatusProvider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn check(&self, client: &reqwest::Client) -> Result<Vec<Incident>, AppError>;
}

#[async_trait]
pub trait Notifier: Send + Sync {
    fn name(&self) -> &'static str;
    async fn notify(&self, client: &reqwest::Client, incident: &Incident) -> Result<(), AppError>;
}
```

### Incident model (normalized across all providers)

| Field | Description |
|-------|-------------|
| `provider` | Provider name (`"azure"`, `"aws"`, etc.) |
| `id` | Unique identifier for deduplication |
| `title` | Incident title |
| `description` | Full description text |
| `link` | URL to the incident details page |
| `occurred_at` | Timestamp string |

## Configuration

All settings via environment variables or `.env` file.

### Global

| Variable | Default | Description |
|----------|---------|-------------|
| `POLL_INTERVAL_MINUTES` | `5` | Minutes between poll cycles (1–1440) |
| `REQUEST_TIMEOUT_SECS` | `30` | HTTP request timeout in seconds (1–300) |
| `STATE_FILE_PATH` | `state.json` | Path to state file for seen-ID tracking |
| `RUST_LOG` | `info` | Log level (tracing subscriber) |

### Providers

| Variable | Default | Description |
|----------|---------|-------------|
| `PROVIDERS` | `azure` | Comma-separated list of enabled providers |
| `PROVIDER_AZURE_FEED_URL` | `https://azure.status.microsoft/en-us/status/feed/` | Azure RSS feed endpoint |
| `PROVIDER_AWS_FEED_URL` | `https://status.aws.amazon.com/rss/all.rss` | AWS RSS feed endpoint |
| `PROVIDER_TWILIO_FEED_URL` | `https://status.twilio.com/history.rss` | Twilio RSS feed endpoint |
| `PROVIDER_AIRSHIP_FEED_URL` | `https://status.airship.com/rss` | Airship RSS feed endpoint |
| `PROVIDER_GITHUB_FEED_URL` | `https://www.githubstatus.com/history.rss` | GitHub RSS feed endpoint |
| `PROVIDER_CLOUDFLARE_FEED_URL` | `https://www.cloudflarestatus.com/history.rss` | Cloudflare RSS feed endpoint |

Provider-specific config follows the pattern `PROVIDER_{NAME}_{KEY}`.

### Notifiers

| Variable | Required | Description |
|----------|----------|-------------|
| `NOTIFIERS` | `telegram` | Comma-separated list of enabled notifiers |
| `NOTIFIER_TELEGRAM_BOT_TOKEN` | yes | Telegram bot token from @BotFather |
| `NOTIFIER_TELEGRAM_CHAT_ID` | yes | Target chat ID |

Notifier-specific config follows the pattern `NOTIFIER_{NAME}_{KEY}`.

### Example `.env`

```bash
PROVIDERS=azure,aws,github,cloudflare
NOTIFIERS=telegram
POLL_INTERVAL_MINUTES=5
REQUEST_TIMEOUT_SECS=30

PROVIDER_AZURE_FEED_URL=https://azure.status.microsoft/en-us/status/feed/

NOTIFIER_TELEGRAM_BOT_TOKEN=123456:ABC-DEF1234gh...
NOTIFIER_TELEGRAM_CHAT_ID=-1001234567890

RUST_LOG=info
```

## Getting a Telegram chat ID

1. Create a bot with [@BotFather](https://t.me/BotFather) — you'll get a token
2. Send any message to your bot in Telegram
3. Visit `https://api.telegram.org/bot<TOKEN>/getUpdates` — the `chat.id` field is your chat ID

## Adding a new provider

Every provider lives in a single file under `src/providers/`. Adding one requires changes in 5 places:

### Step 1: Create `src/providers/<name>.rs`

For an RSS provider, delegate to `rss_common::RssProvider` — the struct, HTTP fetch, RSS parsing, URL validation, and private-IP checks are all provided. You only supply the provider name, allowed domains, and config key:

```rust
use super::rss_common::RssProvider;

// Allowed hostnames for SSRF protection (update for your provider)
const ALLOWED_DOMAINS: &[&str] = &[
    "www.githubstatus.com",
    "githubstatus.com",
];
const CONFIG_KEY: &str = "PROVIDER_GITHUB_FEED_URL";

pub fn new(feed_url: String) -> RssProvider {
    RssProvider::new("github", feed_url, CONFIG_KEY, ALLOWED_DOMAINS)
}

// For integration tests — bypasses URL domain validation
pub fn new_unvalidated(feed_url: String) -> RssProvider {
    RssProvider::new_unvalidated("github", feed_url, CONFIG_KEY, ALLOWED_DOMAINS)
}
```

For a JSON provider (like Twilio), implement `StatusProvider` directly in the file. See `src/providers/twilio.rs` for the full pattern.

### Step 2: Write unit tests (inside the same file)

RSS parsing and generic URL validation (private IPs, schemes, malformed URLs) are already covered by `src/providers/rss_common.rs`. You only need to test the domain-specific allow-list for your provider:

```rust
#[cfg(test)]
mod url_validation_tests {
    use super::*;

    #[test]
    fn validates_allowed_domains() {
        let provider = new("https://www.githubstatus.com/history.rss".into());
        assert!(provider.validate_feed_url().is_ok());
    }

    #[test]
    fn rejects_disallowed_domains() {
        let provider = new("https://evil.example.com/feed/".into());
        let result = provider.validate_feed_url();
        assert!(result.is_err());
        match result.unwrap_err() {
            crate::error::AppError::InvalidConfig { key, value } => {
                assert_eq!(key, "PROVIDER_GITHUB_FEED_URL");
                assert!(value.contains("not in allowed domains"));
            }
            other => panic!("expected InvalidConfig, got {:?}", other),
        }
    }
}
```

Total: **2 unit tests per RSS provider** (domain allow-list only — everything else is tested in `rss_common`).

### Step 3: Register in `src/providers/mod.rs`

Two changes needed:

```rust
// 1. Add module declaration at top of file
pub mod github;

// 2. Add match arm inside build_one()
"github" => {
    let feed_url = crate::config::provider_param("github", "FEED_URL")
        .unwrap_or_else(|| "https://www.githubstatus.com/history.rss".into());
    Ok(Box::new(github::new(feed_url)))
}
```

### Step 4: Add integration tests in `tests/<name>_provider.rs`

Create a new file — one per provider. Import the shared helpers from `tests/common/mod.rs` (`RSS_ITEM_XML`, `build_client`).

3 wiremock-based tests per provider:

```rust
mod common;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wn_alerts::StatusProvider;

#[tokio::test]
async fn fetches_and_parses_rss() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/history.rss"))
        .respond_with(ResponseTemplate::new(200).set_body_string(common::RSS_ITEM_XML))
        .mount(&mock_server)
        .await;

    let feed_url = format!("{}/history.rss", mock_server.uri());
    let provider = wn_alerts::providers::github::new_unvalidated(feed_url);

    let incidents = provider.check(&common::build_client()).await.expect("check should succeed");
    assert_eq!(incidents.len(), 1);
    assert_eq!(incidents[0].id, "integ-test-guid-001");
    assert_eq!(incidents[0].provider, "github");
}

#[tokio::test]
async fn handles_empty_feed() {
    // Mock returns RSS with <channel> but no <item> → assert incidents.is_empty()
}

#[tokio::test]
async fn handles_http_error() {
    // Mock returns 500 → assert result.is_err()
}
```

To run just the new file: `cargo test --test github_provider`

### Step 5: Update configuration in README

Add the provider to these README sections:

- **Line 3** (one-liner): append provider name
- **Architecture** (`src/` tree): add entry under `providers/`
- **Provider variable table**: add `PROVIDER_{NAME}_{KEY}` row with default URL
- **Example `.env`**: add to `PROVIDERS=` list
- **Test count**: bump unit by 2, integration by 3 in the Testing section below

### Provider checklist (before PR)

- [ ] `src/providers/<name>.rs` created with `ALLOWED_DOMAINS`, `CONFIG_KEY`, `new()`, `new_unvalidated()`
- [ ] 2 URL validation unit tests in `url_validation_tests` (`validates_allowed_domains`, `rejects_disallowed_domains`)
- [ ] `pub mod <name>;` added to `src/providers/mod.rs`
- [ ] Factory match arm added in `build_one()` with default URL
- [ ] 3 integration tests in `tests/<name>_provider.rs` (fetch+parse, empty, HTTP error)
- [ ] `cargo test` — all tests pass (expect +2 unit, +3 integration vs current total)
- [ ] `cargo clippy` — zero warnings
- [ ] README updated (5 places listed above)

## Adding a new notifier

1. Create `src/notifiers/<name>.rs` — implement `Notifier`
2. Register in `src/notifiers/mod.rs`
3. Configure via `NOTIFIERS` + `NOTIFIER_{NAME}_*` env vars

## Testing

```bash
cargo test                          # run everything
cargo test --test azure_provider    # run one provider's integration tests
cargo test --test telegram_notifier # run notifier integration tests
```

119 tests: 93 unit (providers, notifiers, state, config, HTML utils, scheduler concurrency) + 26 integration (wiremock-based HTTP tests).

Integration tests live one file per provider/notifier under `tests/`. Shared fixtures (`RSS_ITEM_XML`, `build_client`) are in `tests/common/mod.rs`.

### Concurrency tests

The scheduler's concurrent polling is verified by tests that measure wall-clock time:

```rust
// 2 providers, each with 200ms mock delay → should complete in ~200ms (concurrent)
// not ~400ms (sequential)
#[tokio::test]
async fn fetch_all_providers_concurrently_is_actually_concurrent() { ... }
```

Run with: `cargo test --lib core::scheduler::tests`

## Security

**Key controls:**
- SSRF protection: domain allowlist, private IP blocking, scheme validation, redirect following disabled
- Response size cap: feed responses rejected above 10 MB (Content-Length check + byte-count check)
- Secret handling: bot tokens redacted from logs and error messages, state files 0o600 permissions
- Bounded resource usage: config validation, state ID caps (10k/provider), graceful shutdown
- XML parsing safety: quick-xml with no external entity processing
- Concurrent polling safety: providers wrapped in `Arc<dyn StatusProvider>` for thread-safe sharing across spawned tasks

# To do:
[] Okta is requesting auth
[] f5 domain is gone, need to check in statuspage.io

For now okta and f5 remain disabled
