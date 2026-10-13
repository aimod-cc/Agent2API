# Agent2API · Multi-Provider Local Gateway

[简体中文](./README.md) | **English** | [繁體中文](./README.zh-Hant.md) | [日本語](./README.ja.md) | [한국어](./README.ko.md) | [Português (BR)](./README.pt-BR.md)

Wraps the login state of several AI desktop clients into a local **OpenAI-compatible API gateway**, exposing a single `base_url` and bundling multi-provider account management, model management (enable / disable / delete / alias), content redaction, egress proxying and request reporting — plus a ready-to-run Tauri desktop app. Any OpenAI client that accepts a custom `base_url` can call these providers' model quota through `http://127.0.0.1:3065/v1` — no API key, no client source changes needed.

Reverse-proxy capabilities at a glance (✓ supported · ✗ not supported · — no such concept / not applicable):

| Platform | LLM requests | Token auto-refresh | Model list (remote refresh) | Balance query | Daily check-in | Claims |
| --- | :--: | :--: | :--: | :--: | :--: | :--: |
| WorkBuddy (China) | ✓ | ✓ | ✓ remote + static fallback | ✓ | ✓ daily check-in | — |
| WorkBuddy (Global) | ✓ | ✓ | ✓ remote + static fallback | ✓ | ✗ no check-in program | — |
| Raccoon | ✓ | ✓ | ✓ remote + static fallback | ✓ | ✓ daily desktop-login points | — |
| CatPaw | ✓ | ✗ no refresh flow | ✓ remote + static fallback | ✓ | ✗ | — |
| AutoClaw (domestic / international) | ✓ | ✓ | ✓ remote + static fallback | ✓ | ✓ daily check-in | — |
| Qoder | ✓ | ✓ | ✓ remote (per region) + static fallback | ✓ | ✓ China only | — |
| Cline (Free / Pass) | ✓ | ✓ | ✓ remote + static fallback | ✓ | — | — |
| Accio (international / domestic) | ✓ | ✓ | ✓ remote + static fallback | ✓ used-percent only | — | — |
| ZCode (domestic / international) | ✓ | ✗ | ✗ static table | ✓ plan balance | — | ✓ timed plan (manual) |
| CodeArts | ✓ | ✓ one-shot rotation | ✓ remote (three sources merged) | ✓ two ledgers | — | ✓ daily welfare (manual) |
| Trae | ✓ | ✓ single-use rotation | ✓ remote only | ✓ two ledgers | — | — |
| Loomy (iFlytek) | ✓ | ✗ no refresh flow | ✓ remote only | ✓ two point ledgers | ✓ daily gifted-points refresh | — |
| KukuAI (Baidu Wenku) | ✓ | ✗ no refresh flow | ✓ remote + static fallback | ✓ points balance | ✓ daily check-in (free points) | — |
| MonkeyCode (Chaitin, domestic / international) | ✓ | ✗ no refresh flow | ✓ remote only | — | — | — |
| Command Code | ✓ | ✗ static API key | ✓ remote + static fallback | — | — | — |
| Antigravity (Google, Gemini) | ✓ | ✓ | ✓ remote + static fallback | — | — | — |
| OfficeAce (Huawei Cloud, OfficeClaw) | ✓ | ✓ control-plane auto-refresh | ✓ remote only | ✓ points balance | ✓ daily bonus (check-in + newcomer gift) | — |
| Custom providers | ✓ chat passthrough / Responses / Anthropic | — | ✓ manual + server-side fetch | — | — | — |

The three chat entry points (`/v1/chat/completions`, `/v1/responses`, `/v1/messages`, plus `/v1/messages/count_tokens`) and `/v1/models` behave identically for every platform — the differences above are only about what each upstream can do. Model mapping, the global priority queue, 429 fallback, egress proxying, content redaction and request reporting apply to all platforms alike.

> **This project is for learning and discussion only.** It reuses your own accounts' login state in the shape of a non-official client, which may violate the upstream services' terms; risks (including rate limiting or account bans) are your own. Commercial use and circumventing billing are prohibited. See [Usage Notice](#usage-notice) and [LICENSE](./LICENSE).
>
> This is a personal, local-purpose proxy tool, unaffiliated with any upstream vendor or their official products (the list is in the [Usage Notice](#usage-notice)); interface shapes come from observing the clients' traffic, and upstream may change at any time.

---

## Table of Contents

- [Quick Start](#quick-start)
- [Docker Deployment](#docker-deployment)
- [Screenshots](#screenshots)
- [Project Layout](#project-layout)
- [Development & Build](#development--build)
- [Usage Notice](#usage-notice)
- [License](#license)

---

## Quick Start

Download the installer from Releases (NSIS, Simplified Chinese, installs to `C:\Program Files\Agent2API` by default, and needs administrator approval during setup), then launch it — **no Node or any other runtime required**.

1. First launch starts the local gateway (port 3065) inside the app process and opens the main window. If an older version's data directory or data files are found, a dialog walks you through the migration.
2. Click "Add account" on the Accounts page, pick a provider and follow the dialog: sign in or fill in credentials (web login / SMS code / pasted credentials / importing this machine's desktop login state).
3. Set your OpenAI client's `base_url` to `http://127.0.0.1:3065/v1` and put anything in `api_key` (for example `sk-local`; the server does not check it while authentication is disabled).

Closing the window minimizes to the tray by default and the gateway keeps forwarding in the background; to quit for real, right-click the tray icon and choose "Exit".

### Verification

Once the gateway is up, use curl to confirm connectivity (put any name that actually exists in `GET /v1/models` into `model`):

```bash
curl http://127.0.0.1:3065/health
curl http://127.0.0.1:3065/v1/models

curl http://127.0.0.1:3065/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"deepseek-v4.1-flash","messages":[{"role":"user","content":"Hello"}],"stream":true}'
```

Client example (Python SDK):

```python
from openai import OpenAI

client = OpenAI(base_url="http://127.0.0.1:3065/v1", api_key="sk-local")
resp = client.chat.completions.create(
    model="deepseek-v4.1-flash",     # whichever provider owns it — see GET /v1/models
    messages=[{"role": "user", "content": "Hello"}],
)
print(resp.choices[0].message.content)
```

**Pages running in a browser** (a self-built web UI, …) that call this endpoint with `fetch` hit a cross-origin preflight rejection — the gateway does **not** answer CORS by default, so the preflight lands on the API-key check and gets a 401. Two ways out: ① turn on "Security → Gateway CORS" in Settings (origin `*`; with `*` any web page could drive your local gateway, so set a "Gateway Key" too); ② make the page same-origin — serve it from a small local static server that reverse-proxies `/v1` to `127.0.0.1:3065`.

### LAN access

By default the gateway only listens on `127.0.0.1`. After turning on "Settings → General → LAN access" it listens on all adapters, and devices on the LAN just point their API base URL at this machine's IP (the UI shows the full address) to share the accounts. For safety, enabling it requires registering a panel administrator first: the management API then demands an admin session or a gateway key, and with no key enabled the forwarding API refuses service too (the enable flow adds a "default" key automatically). The web panel can optionally be exposed to the LAN as well (off by default). Changes take effect after a restart.

---

## Docker Deployment

```bash
docker run -d --name agent2api --restart unless-stopped \
  -p 3065:3065 -v ./data:/data \
  aimodcc/agent2api:latest
```

Open `http://<host>:3065` in a browser — the first visit walks you through **registering the admin account**; log in and create an API key in the "Gateway Keys" page for your clients — `http://<host>:3065/v1` is the OpenAI-compatible endpoint (it refuses to forward until the first key exists, then recovers automatically). All state (SQLite database / config / logs) lives in the `./data` volume.

Compose users (images are published for amd64 and arm64):

```yaml
services:
  agent2api:
    image: aimodcc/agent2api:latest
    container_name: agent2api
    restart: unless-stopped
    ports:
      - "3065:3065"
    volumes:
      - ./data:/data
```

Environment variables (all optional — nothing needs to be preset):

| Variable | Description |
| --- | --- |
| `AGENT2API_ADMIN_USER` + `AGENT2API_ADMIN_PASSWORD` | Preset the admin account & password (password in plain text, hashed automatically at startup). Leave unset to register in the panel |
| `AGENT2API_PANEL_PORT` | Serve the panel (UI + `/api/*`) on its own port; map only the main port publicly to keep the management plane internal (bind the panel port as `127.0.0.1:3066:3066`) |
| `AGENT2API_HOST` / `AGENT2API_PROXY_PORT` | Listen address (default `0.0.0.0`) / port (default `3065`) |
| `AGENT2API_ALLOW_NO_KEY` | Set to `1` to serve `/v1` without any key — private networks only |
| `AGENT2API_CAPTCHA_ENABLED` | Login-page human verification widget: `1` enabled (default), `0` disabled |

Build from source: clone the repo and run `docker compose up -d --build` (the image contains only the gateway and the panel, no Rust toolchain).

**Web panel capability notes** (all stem from having no local desktop client): web login (WorkBuddy / Qoder / Cline), SMS codes and pasted credentials work fully; web logins that need a callback on this machine (AutoClaw / CatPaw / Accio / CodeArts / Trae) and "import desktop login state" are unavailable — use pasted credentials instead.

---

## Screenshots

### Accounts

All accounts share one queue (the second column is the priority) and can be toggled individually; per-model cooldowns, expiry and balance are shown inline, and low-balance accounts are skipped below the threshold by default.

![Accounts page: global queue, per-model rate-limit cooldown, expiry and balance](./assets/screenshots/accounts.png)

Pick a provider, then sign in; one vendor can hold several account versions (e.g. WorkBuddy China / Global), and forwarding picks the right one by model name:

![Add account: choose provider and edition, then sign in on the web](./assets/screenshots/add-account.png)

### Report

Overview: requests / success rate / tokens / top model, with account and provider rankings, usage donuts and a 365-day activity heatmap:

![Report overview: stat cards, top accounts / providers, model and provider usage donuts](./assets/screenshots/report-overview.png)

Trends: 24-hour **cache hit rate** (line) and **token consumption** (area) on twin axes, with a per-day token bar chart below:

![Report trends: dual-axis cache hit rate and token consumption, per-day token bars](./assets/screenshots/report-trends.png)

### Scheduled Tasks

All background tasks live on one page (toggle / interval / last result / run now); the list is stored in `scheduledTasks` in `~/.agent2api/config.json` and edits apply immediately.

![Scheduled tasks page: toggles and intervals for check-in, credential maintenance, model catalog refresh and more](./assets/screenshots/scheduled-tasks.png)

---

## Project Layout

Both the gateway and the desktop app live under `desktop-tauri/`: the backend is a Rust in-process HTTP server under `src-tauri/`, the frontend is plain HTML/CSS/JS under `ui/`.

```
agent2api/
├─ desktop-tauri/
│  ├─ src-tauri/
│  │  ├─ server/                 Gateway crate (agent2api-server, built independently:
│  │  │                          shared by the desktop app and the headless binary;
│  │  │                          src/server/ and bin/agent2api-server.rs have no GUI deps)
│  │  │  ├─ mod.rs               Service assembly: ServerState, startup, shutdown, startup migration
│  │  │  ├─ http.rs              Route table, CORS, API Key middleware, body limit, headless static hosting
│  │  │  ├─ config.rs / logging.rs / logs_store.rs / errors.rs
│  │  │  ├─ config_migration.rs  1.x config directory migration (~/.workbuddy-proxy → ~/.agent2api, first startup step)
│  │  │  ├─ request_stats.rs + request_stats/   Statistics time windows, writes, aggregation and trimming
│  │  │  ├─ core/
│  │  │  │  ├─ providers/        ★ Multi-provider layer (the heart of this work)
│  │  │  │  │  ├─ mod.rs        ProviderKind (built-in vendors; Cline split into two pools, Accio and
│  │  │  │  │  │                ZCode each into two regions) + PROVIDERS registry + id lookups
│  │  │  │  │  ├─ adapter.rs    ProviderAdapter trait + adapter_for + implemented_kinds
│  │  │  │  │  ├─ router.rs     Model name → set of candidate providers (aggregate catalog)
│  │  │  │  │  ├─ catalog.rs    Aggregate model catalog (list merging / same-name dedup / availability)
│  │  │  │  │  ├─ catalog_cache.rs  Persistent cache of each provider's remote list (restored on restart instead of falling back to the built-in list)
│  │  │  │  │  ├─ refresh_flight.rs  Single-flight dedup for credential refresh
│  │  │  │  │  ├─ workbuddy.rs  WorkBuddy adapter (header set / system injection / 6004 / 11128)
│  │  │  │  │  ├─ raccoon/      Raccoon: mod / models / credentials / jwt / oauth / balance
│  │  │  │  │  ├─ catpaw/       CatPaw: adapter (is_stateful) / conversation (turn state machine) /
│  │  │  │  │  │                turn_executor / prepare / decision (turn decisions) / fingerprint /
│  │  │  │  │  │                registry/ (session registry: table and handles / account identity / invalidation) /
│  │  │  │  │  │                messages / blocks / tools / openai (translation layer) /
│  │  │  │  │  │                upstream_http / image_compress / models / credentials / balance
│  │  │  │  │  ├─ autoclaw/     Zhipu autoglm (domestic + international): region (per-region
│  │  │  │  │  │                domains and identity) / adapter / credentials / refresh / crypto / models /
│  │  │  │  │  │                balance / login (SMS code, domestic only) /
│  │  │  │  │  │                oauth (Zai / Google web login, international only) / checkin (daily check-in task)
│  │  │  │  │  ├─ qoder/        Qoder: adapter / endpoints (both sites) / oauth (device authorization) /
│  │  │  │  │  │                auth / cosy (COSY signing and body encoding) / protocol (envelope decoding) /
│  │  │  │  │  │                chat (session-style forwarding) / stream / machine (PKCE and machine id) /
│  │  │  │  │  │                credentials / refresh / models / balance
│  │  │  │  │  ├─ cline/        Cline: adapter (Bearer + product headers) / credentials (workos: prefix
│  │  │  │  │  │                + desktop login state + name parsing) / login (WorkOS device authorization) /
│  │  │  │  │  │                refresh (single-flight renewal) / models (two quota pools + default mapping seeds) /
│  │  │  │  │  │                balance (credit balance, micro-credit ÷1e6)
│  │  │  │  │  ├─ accio/        Accio (international + domestic): endpoints (both regions and paths) /
│  │  │  │  │  │                credentials / auth / refresh (single-flight) /
│  │  │  │  │  │                oauth (PKCE web login + loopback callback) /
│  │  │  │  │  │                models (static fallback + /api/llm/config/v2) /
│  │  │  │  │  │                protocol (OpenAI <-> ADK Gemini-style envelope) /
│  │  │  │  │  │                chat (session-style forwarding) / stream (ADK SSE unwrapping) / balance
│  │  │  │  │  ├─ zcode/        ZCode (Zhipu Z.AI, domestic + international): region (two regions and identities) /
│  │  │  │  │  │                adapter (stateless, parameterized by region) / credentials (token + plan JWT) /
│  │  │  │  │  │                oauth (CLI polling login) / coding_key (exchange for an inference API key) / models (static table) /
│  │  │  │  │  │                balance (plan balance) / plan + claim (plan channel and timed claim) /
│  │  │  │  │  │                captcha (human-verification token pool) / reasoning (GLM-5.3 thinking budget) /
│  │  │  │  │  │                zcode_system.json (system prompt)
│  │  │  │  │  ├─ codearts/     CodeArts (Huawei Cloud): signer (Huawei Cloud SDK-HMAC-SHA256,
│  │  │  │  │  │                byte-for-byte vectors from the reference implementation) /
│  │  │  │  │  │                credentials / dpop (ES256 DPoP proof) /
│  │  │  │  │  │                oauth (PKCE web login + loopback callback) / refresh (single-flight) /
│  │  │  │  │  │                session (chat-session heartbeat + per-account admission) /
│  │  │  │  │  │                chat (session-style forwarding) /
│  │  │  │  │  │                stream_fault (error envelopes inside HTTP 200 SSE) /
│  │  │  │  │  │                redact (credential scrubbing in error bodies) /
│  │  │  │  │  │                models (agent / builtin / benefit-gateway merge) /
│  │  │  │  │  │                balance (subscription statistics + benefit pool, two accounts) /
│  │  │  │  │  │                welfare (daily claim: idempotency key persisted first, verified after)
│  │  │  │  │  ├─ trae/         Trae (ByteDance AI IDE, SOLO channel): credentials / device (device keypair) /
│  │  │  │  │                   login + oauth (PKCE web login + token-exchange candidates) / callback_server
│  │  │  │  │                   (random local port callback with noise filtering) / refresh (single-flight) /
│  │  │  │  │                   payload (SOLO envelope rebuilt from a whitelist) / headers (SOLO header set) /
│  │  │  │  │                   stream (SSE -> chunk translation) / forward (stateful forwarding) /
│  │  │  │  │                   errors (error classification and dead-config list) /
│  │  │  │  │                   models (get_detail_param catalog) /
│  │  │  │  │                   usage (entitlement packs + plan quota, two accounts) / profile (identity parsing)
│  │  │  │  │  ├─ loomy/        Loomy (iFlytek): login (SMS code) / credentials (14-day session, no
│  │  │  │  │  │                refresh flow) / sign (client HMAC-SHA1 headers) / endpoints /
│  │  │  │  │  │                client (integration gateway) / models (/api/v1/models remote only,
│  │  │  │  │  │                no built-in fallback) / balance (permanent + daily gifted points) /
│  │  │  │  │  │                checkin (first login of the day refreshes the gifted points)
│  │  │  │  │  ├─ kuku/         KukuAI (Baidu Wenku "Kuku AI / GenFlowPro", kuku.baidu.com):
│  │  │  │  │  │                adapter (is_stateful) / session (bdstoken/uinfo/uk trio cache) /
│  │  │  │  │  │                engine (STOKEN exchange) / http / login (main-site web login + shell-side cookie capture) /
│  │  │  │  │  │                credentials (BDUSS cookies) / models (static fallback + remote refresh) /
│  │  │  │  │  │                chat (create session → allocate compute → SSE) / balance (points) /
│  │  │  │  │  │                checkin (daily free points)
│  │  │  │  │  ├─ monkeycode/   MonkeyCode (Chaitin, domestic + international): region (two sites) /
│  │  │  │  │  │                adapter / endpoints (paths, cookie name, interface_type -> CLI map) /
│  │  │  │  │  │                client / credentials (session + imageId) / login (paste session and discovery) /
│  │  │  │  │  │                models (one slot per site, remote only) / task (create task) /
│  │  │  │  │  │                stream (WS task stream) / translate (ACP events -> chat frames, with
│  │  │  │  │  │                auto-approval of tools and auto-reply to agent questions)
│  │  │  │  │  ├─ commandcode/  Command Code: adapter / endpoints / credentials / login /
│  │  │  │  │  │                fingerprint (deterministic device fingerprint + 8h reporting) / models /
│  │  │  │  │  │                plan (8-key envelope, params rewrite, session id derivation)
│  │  │  │  │  ├─ antigravity/  Antigravity (Google, Gemini): oauth (refresh token + single-flight) /
│  │  │  │  │                   project (loadCodeAssist -> onboardUser discovery) / credentials /
│  │  │  │  │                   endpoints (three environment bases and header set) / login /
│  │  │  │  │                   models (fetchAvailableModels + built-in fallback + real-name mapping) / adapter
│  │  │  │  │  └─ officeace/    OfficeAce (Huawei Cloud, OfficeClaw): oauth (PKCE + P-256 DPoP self-service login) /
│  │  │  │  │                   signer (both SDK-HMAC and V11-HMAC signatures + vector fixtures) /
│  │  │  │  │                   credentials (gateway Basic + control-plane temporary AK/SK) /
│  │  │  │  │                   chat (stateless OpenAI-compatible) / models (remote catalog + per-model multipliers) /
│  │  │  │  │                   probe (availability probe; unreachable models hidden by default) /
│  │  │  │  │                   subscription (subscription / quota / rewards) / onboarding / checkin / balance
│  │  │  │  ├─ protocol/        Protocol translation layer (per-vendor wire <-> standard chat SSE request/response
│  │  │  │  │                   translation: Anthropic Messages / Responses / NDJSON (Command Code) /
│  │  │  │  │                   Gemini envelope (Antigravity), plus tool plans and history repair)
│  │  │  │  ├─ upstream/        Forwarding orchestration: global account queue loop (provider_loop) + request body
│  │  │  │  │                   handling (payload) + SSE passthrough/aggregation + usage side-channel extraction +
│  │  │  │  │                   translation stream wiring for non-chat protocols (translate: three in parallel)
│  │  │  │  ├─ account_store/   Account storage (global priority, rate-limit cooldown, per-vendor add and import)
│  │  │  │  ├─ models/          Model catalog internals (built-in WorkBuddy list + /v3/config refresh)
│  │  │  │  ├─ model_rules.rs   Model management rules (disable / hide / mapping alias)
│  │  │  │  ├─ api_keys.rs      Gateway key list (multiple keys, any enabled one passes)
│  │  │  │  ├─ auth.rs / auth_http.rs / login.rs   Sessions, egress transport, headless login
│  │  │  │  │                    (login/ holds the vendor-specific flows: CatPaw's
│  │  │  │  │                    loopback callback, Qoder's device authorization)
│  │  │  │  ├─ routing.rs / billing/   Account routing (global priority + rate-limit cooldown) / points check-in ops
│  │  │  │  ├─ proxies.rs / clash.rs / egress.rs   Egress proxies and a per-exit cached Client
│  │  │  │  ├─ sanitize.rs      Outbound fingerprint sanitization (header stripping + minimal rewrites)
│  │  │  │  ├─ prompt.rs        Gateway-owned system prompt (passthrough / replace / append)
│  │  │  │  ├─ degrade.rs       Content-block degradation state (neutral prompt until next 00:00)
│  │  │  │  ├─ credential_maintenance.rs  Batch refresh of expired / soon-to-expire credentials
│  │  │  │  ├─ usage_query.rs     Balance / points queries (concurrent across accounts + the snapshot taken by the scheduled run)
│  │  │  │  ├─ scheduled_tasks.rs  Interval-based scheduled task registry and dispatch loop (toggle / interval /
│  │  │  │  │                       last result; configured under scheduledTasks in config.json)
│  │  │  │  └─ account_transfer.rs + account_transfer/ / auto_checkin.rs / update/
│  │  │  │                       Import/export (with identity normalization) / scheduled check-in / software updates
│  │  │  └─ api/                 Per-route handlers (health/session/accounts/accounts_usage/
│  │  │                          chat/models/keys/model_manage/stats/logs/billing/
│  │  │                          sanitize/prompt/auto-checkin/scheduled-tasks/update/…)
│  │  ├─ lib.rs                  App entry point (config directory migration → settings → tray → main window → start backend)
│  │  ├─ backend.rs              In-process server lifecycle
│  │  ├─ legacy_install.rs       Cleanup of the old "current user" install (directory / shortcuts / uninstall entry / autostart; release only)
│  │  ├─ gateway.rs              Shell-side HTTP client for the management API
│  │  ├─ login.rs / commands.rs  Login window and polling, invoke commands exposed to the frontend
│  │  ├─ login_profile.rs        Each web login gets its own temporary WebView2 data directory (deleted when done)
│  │  ├─ bridge.rs               Bridge script injected as window.workbuddyDesktop
│  │  └─ update.rs / settings.rs / state.rs / tray.rs
│  ├─ ui/                        Frontend (plain HTML/CSS/JS, no framework)
│  └─ src-tauri/tauri.conf.json  Bundle configuration (NSIS)
├─ build/make-icon.mjs           Generates the app icon source image
├─ assets/screenshots/           Images used by the READMEs (UI screenshots)
├─ Dockerfile / .dockerignore    Headless image (multi-stage build: gateway + panel only)
├─ docker-compose.yml / .env.example   Deployment (single container: panel + gateway on one port)
└─ package.json                  Build script entry points (tauri:dev / tauri:build / build:icon)
```

---

## Development & Build

### Requirements

- Rust >= 1.77 and the Tauri 2 toolchain (to compile the desktop app itself; Windows also needs the WebView2 runtime)
- Node.js >= 18.17 (only to run `npm run tauri:*` and frontend build scripts such as `build/make-icon.mjs`; the desktop app does not depend on Node at runtime and ships no Node artifacts)

### Common scripts

```bash
npm run tauri:install      # Install desktop dependencies (same as npm --prefix desktop-tauri install)
npm run tauri:dev          # Launch the desktop app in dev mode (with hot reload)
npm run tauri:build        # Build the desktop installer

npm run build:icon         # Generate the icon source image (run after changing the icon design, then run tauri icon)
```

The root project has no runtime dependencies; `package.json` only provides the shortcut script entry points above. The build artifact is `target/release/bundle/nsis/Agent2API_<version>_x64-setup.exe` (currently about 3.0 MB; `src-tauri/.cargo/config.toml` points cargo's `target-dir` at the project root's `target/`).

---

## Usage Notice

### For learning and discussion only

This project is a hands-on exercise in HTTP reverse proxying, SSE streaming passthrough, multi-upstream protocol adaptation and desktop packaging (Tauri), and is **for personal learning and research only**. It is not an official product and has no affiliation with, endorsement from or sponsorship by Tencent and WorkBuddy / CodeBuddy, Meituan and CatPaw, SenseTime and Raccoon, Zhipu and AutoClaw / autoglm, Alibaba and Qoder / Accio, Huawei Cloud and CodeArts / OfficeAce, ByteDance and Trae, iFlytek and Loomy, Baidu Wenku and KukuAI, Chaitin and MonkeyCode, Command Code, or Google and Antigravity.

### About the reverse-proxy behaviour

What this project implements is a local reverse proxy: it reuses your own accounts' login state on your own machine and forwards requests to the official upstream gateways. It does not crack or bypass any payment or permission check — the quota it uses always comes from what your own account already has. That said, forwarding in the shape of a non-official client **may violate the upstream services' user agreements or terms of use**; whether to use it, and every consequence that follows (including but not limited to rate limiting, risk-control flags, account suspension or bans), is borne by the user.

### Prohibited uses

Do not use this project for any commercial purpose, for redistributing it for profit, for bulk account operation, for circumventing upstream billing or quota limits, or for any activity that violates local laws and regulations. To call these model services in production or commercial settings, use the official channels and official APIs.

### Credentials and data risk

This project stores account credentials (`accessToken` / `refreshToken`, etc.) as **plain text** in the local configuration directory (by default `~/.agent2api/`), and files produced by the export feature contain plain-text credentials as well. Keep them safe: never commit them to a public repository, upload them to cloud storage or share them with others. Losses caused by leaked credentials are borne by the user.

### No warranty and rights notice

This project is provided "as is"; the author makes no promise about its availability, stability, security or fitness for a particular purpose. Upstream interfaces may change at any time and the project may stop working and go unmaintained at any time. The full terms are in [LICENSE](./LICENSE). The interface shapes, protocol fields and other information in this project come from observing and organizing publicly visible network traffic of the official clients; all related trademarks and services belong to their respective owners. If a rights holder believes this project is inappropriate, please contact the author and it will be adjusted or removed promptly.

---

## License

This project is released under the [MIT License](./LICENSE); you may use, modify and distribute it freely as long as the copyright notice is retained.

One caveat: the LICENSE file carries a **Usage Notice** after the MIT text, whose clause 3 **adds restrictions on top of** MIT (no commercial use, no reselling redistributions, no bulk account operation). This project is therefore **not** pure MIT — **the MIT terms and the Usage Notice together form the complete license**, and where the two reach different conclusions on the same act, the stricter one governs. That is also why `Cargo.toml` points `license-file` at the LICENSE file instead of declaring the SPDX identifier `"MIT"`.

---

## Star History

<a href="https://star-history.com/#aimod-cc/agent2api&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=aimod-cc/agent2api&type=Date&theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=aimod-cc/agent2api&type=Date" />
    <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=aimod-cc/agent2api&type=Date" />
  </picture>
</a>

