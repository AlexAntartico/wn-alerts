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

1. **Backoff check**: any provider currently in backoff is skipped and its remaining cycle count is decremented. A `WARN` log is emitted for each skipped provider. Providers not in backoff proceed to the next phase.
2. **Fan-out phase**: Active providers are queried **concurrently** via `tokio::spawn` — a slow or unreachable provider doesn't block others. Wall-clock tick time is `max(provider_times)` instead of `sum(provider_times)`. For 7 providers at 30s timeout each, that's **30 seconds max** instead of 3.5 minutes.
3. **Fan-in phase**: Results are processed sequentially — new incidents (not previously seen) are routed to every enabled notifier, and marked seen in state
4. An incident is marked seen **only after at least one notifier succeeds**. If all notifiers fail, the incident is left unseen and will retry on the next cycle — failure is logged as a warning
5. Seen incident IDs are persisted to `state.json` after every cycle so alerts only fire once per incident

**SIGINT** (Ctrl+C) and **SIGTERM** both trigger a graceful shutdown with state save. SIGTERM support means `systemctl stop` and `systemctl restart` are safe — state is always persisted before exit.

### Provider backoff

By default every provider is polled every cycle regardless of errors. Setting `PROVIDER_FAILURE_THRESHOLD` enables exponential backoff for persistently failing providers:

- After **N consecutive failures** the provider is skipped for `2^N` cycles (capped at `PROVIDER_BACKOFF_MAX_CYCLES`)
- Each skipped cycle decrements the counter and emits a `WARN` log: `Provider is backing off — skipping this cycle cycles_remaining=N`
- The **first successful poll resets** both the failure counter and any remaining backoff
- Backoff state is persisted to `state.json` so a daemon restart does not reset the budget

Example with `PROVIDER_FAILURE_THRESHOLD=3` and a 5-minute poll interval:

| Consecutive failures | Backoff cycles | Approx. skip time |
|---|---|---|
| 3 (threshold hit) | 8 | 40 min |
| 4 | 16 | 1.3 hr |
| 5 | 32 (default cap) | 2.7 hr |
| 6+ | 32 (capped) | 2.7 hr |

Without this setting the daemon keeps retrying dead providers every cycle, producing one `ERROR` log per poll indefinitely. Enabling backoff reduces that to a single `ERROR` burst followed by quiet `WARN` skips.

```bash
May 29 12:33:51 sg1-vps-dev wn-alerts[691]: 2026-05-29T12:33:51.533707Z  INFO wn_alerts::core::scheduler: --- Poll cycle start ---
May 29 12:33:51 sg1-vps-dev wn-alerts[691]: 2026-05-29T12:33:51.535575Z  INFO wn_alerts::core::scheduler: Checking status... provider="azure"
May 29 12:33:51 sg1-vps-dev wn-alerts[691]: 2026-05-29T12:33:51.535777Z  INFO wn_alerts::core::scheduler: Checking status... provider="aws"
May 29 12:33:51 sg1-vps-dev wn-alerts[691]: 2026-05-29T12:33:51.535862Z  INFO wn_alerts::core::scheduler: Checking status... provider="cloudflare"
May 29 12:33:51 sg1-vps-dev wn-alerts[691]: 2026-05-29T12:33:51.535945Z  INFO wn_alerts::core::scheduler: Checking status... provider="twilio"
May 29 12:33:51 sg1-vps-dev wn-alerts[691]: 2026-05-29T12:33:51.536022Z  INFO wn_alerts::core::scheduler: Checking status... provider="imperva"
May 29 12:33:51 sg1-vps-dev wn-alerts[691]: 2026-05-29T12:33:51.536089Z  INFO wn_alerts::core::scheduler: Checking status... provider="airship"
May 29 12:33:51 sg1-vps-dev wn-alerts[691]: 2026-05-29T12:33:51.536153Z  INFO wn_alerts::core::scheduler: Checking status... provider="github"
May 29 12:33:52 sg1-vps-dev wn-alerts[691]: 2026-05-29T12:33:52.105533Z  INFO wn_alerts::core::scheduler: Fetched 47 incident(s) provider="airship" total=47
May 29 12:33:52 sg1-vps-dev wn-alerts[691]: 2026-05-29T12:33:52.105982Z  INFO wn_alerts::core::scheduler: Fetched 36 incident(s) provider="aws" total=36
May 29 12:33:52 sg1-vps-dev wn-alerts[691]: 2026-05-29T12:33:52.106048Z  INFO wn_alerts::core::scheduler: Fetched 2 incident(s) provider="azure" total=2
May 29 12:33:52 sg1-vps-dev wn-alerts[691]: 2026-05-29T12:33:52.106058Z  INFO wn_alerts::core::scheduler: New incident detected provider="azure" id=active-azure-openai-service-elevated-error-rates-in-multiple-regions title=Active - Azure OpenAI Service Elevated Error Rates in multiple regions
May 29 12:33:53 sg1-vps-dev wn-alerts[691]: 2026-05-29T12:33:53.505374Z  INFO wn_alerts::core::scheduler: Notification sent notifier="telegram" provider=azure
May 29 12:33:53 sg1-vps-dev wn-alerts[691]: 2026-05-29T12:33:53.506106Z  INFO wn_alerts::core::scheduler: Fetched 25 incident(s) provider="cloudflare" total=25
May 29 12:33:53 sg1-vps-dev wn-alerts[691]: 2026-05-29T12:33:53.506193Z  INFO wn_alerts::core::scheduler: Fetched 25 incident(s) provider="github" total=25
May 29 12:33:53 sg1-vps-dev wn-alerts[691]: 2026-05-29T12:33:53.506235Z  INFO wn_alerts::core::scheduler: Fetched 25 incident(s) provider="imperva" total=25
May 29 12:33:53 sg1-vps-dev wn-alerts[691]: 2026-05-29T12:33:53.506295Z  INFO wn_alerts::core::scheduler: Fetched 25 incident(s) provider="twilio" total=25
May 29 12:33:53 sg1-vps-dev wn-alerts[691]: 2026-05-29T12:33:53.506822Z  INFO wn_alerts::core::scheduler: --- Poll cycle complete ---
```

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
| `PROVIDER_AZURE_FEED_URL` | `https://rssfeed.azure.status.microsoft/en-us/status/feed/` | Azure RSS feed endpoint |
| `PROVIDER_AWS_FEED_URL` | `https://status.aws.amazon.com/rss/all.rss` | AWS RSS feed endpoint |
| `PROVIDER_TWILIO_FEED_URL` | `https://status.twilio.com/history.rss` | Twilio RSS feed endpoint |
| `PROVIDER_AIRSHIP_FEED_URL` | `https://status.airship.com/rss` | Airship RSS feed endpoint |
| `PROVIDER_GITHUB_FEED_URL` | `https://www.githubstatus.com/history.rss` | GitHub RSS feed endpoint |
| `PROVIDER_CLOUDFLARE_FEED_URL` | `https://www.cloudflarestatus.com/history.rss` | Cloudflare RSS feed endpoint |
| `PROVIDER_IMPERVA_FEED_URL` | `https://status.imperva.com/history.rss` | Imperva RSS feed endpoint |

#### Provider backoff (optional)

| Variable | Default | Description |
|----------|---------|-------------|
| `PROVIDER_FAILURE_THRESHOLD` | unset | Consecutive failures before backoff starts. Unset = disabled, poll every cycle |
| `PROVIDER_BACKOFF_MAX_CYCLES` | `32` | Maximum cycles to skip when in backoff (ignored if threshold unset). At 5 min intervals, 32 cycles ≈ 2.7 hours |

Provider-specific config follows the pattern `PROVIDER_{NAME}_{KEY}`.

### Notifiers

| Variable | Required | Description |
|----------|----------|-------------|
| `NOTIFIERS` | `telegram` | Comma-separated list of enabled notifiers |
| `NOTIFIER_TELEGRAM_BOT_TOKEN` | yes | Telegram bot token from @BotFather |
| `NOTIFIER_TELEGRAM_CHAT_ID` | yes | Target chat ID |

Notifier-specific config follows the pattern `NOTIFIER_{NAME}_{KEY}`.

**Telegram Notification**

<img width="415" height="415" alt="image" src="https://github.com/user-attachments/assets/9d23ffe1-91ed-4989-ac59-3307ab6b579e" />


### Example `.env`

```bash
PROVIDERS=azure,aws,github,cloudflare
NOTIFIERS=telegram
POLL_INTERVAL_MINUTES=5
REQUEST_TIMEOUT_SECS=30

PROVIDER_AZURE_FEED_URL=https://azure.status.microsoft/en-us/status/feed/

# Optional: back off after 3 consecutive failures, skip up to 32 cycles (~2.7 hr at 5 min interval)
# PROVIDER_FAILURE_THRESHOLD=3
# PROVIDER_BACKOFF_MAX_CYCLES=32

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

// Lock to the exact hostname(s) that serve the feed — no broader parent domains.
const ALLOWED_DOMAINS: &[&str] = &["status.githubstatus.com"];
const CONFIG_KEY: &str = "PROVIDER_GITHUB_FEED_URL";

pub fn new(feed_url: String) -> RssProvider {
    RssProvider::new("github", feed_url, CONFIG_KEY, ALLOWED_DOMAINS)
}

// For integration tests — bypasses URL domain validation
pub fn new_unvalidated(feed_url: String) -> RssProvider {
    RssProvider::new_unvalidated("github", feed_url, CONFIG_KEY, ALLOWED_DOMAINS)
}
```

The subdomain rule (`ends_with(".githubstatus.com")`) is applied automatically, so listing `www.githubstatus.com` explicitly is redundant — the bare domain entry is enough to cover it. Only list additional entries when the feed genuinely moves between distinct hostnames (e.g. Azure's `azure.status.microsoft` vs `status.azure.com`).

For a JSON provider, implement `StatusProvider` directly in the file. See `src/providers/twilio.rs` for the full pattern.

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

143 tests: 117 unit (providers, notifiers, state, config, HTML utils, scheduler) + 26 integration (wiremock-based HTTP tests).

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

### Testing graceful shutdown

The scheduler's graceful shutdown path (state saved on signal) is tested via `run_with_shutdown`, a private method that accepts any `Future<Output = &'static str>` as the shutdown trigger. In production, `run()` passes `shutdown_signal()` (which waits for SIGINT or SIGTERM). In tests, pass an immediately-resolving future to exercise the shutdown path without OS signals:

```rust
// Simulates receiving SIGTERM — no real signal sent to the process
scheduler
    .run_with_shutdown(std::future::ready("SIGTERM"))
    .await
    .unwrap();

assert!(std::path::Path::new(&state_file).exists());
```

`run_with_shutdown` is accessible from unit tests inside `src/core/scheduler.rs` (same module, `use super::*`). It is not part of the public API.

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
