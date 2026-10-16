<div align="center">

# Agent2API

**Turn the login state of multiple AI desktop clients into a local OpenAI-compatible API gateway**

[简体中文](../README.md) | **English** | [繁體中文](./README.zh-Hant.md) | [日本語](./README.ja.md) | [한국어](./README.ko.md) | [Português (BR)](./README.pt-BR.md)

[![license](https://img.shields.io/badge/license-MIT%20%2B%20Notice-green)](../LICENSE)
[![release](https://img.shields.io/github/v/release/aimod-cc/agent2api?include_prereleases)](https://github.com/aimod-cc/agent2api/releases/latest)
[![docker pulls](https://img.shields.io/docker/pulls/aimodcc/agent2api)](https://hub.docker.com/r/aimodcc/agent2api)
[![stars](https://img.shields.io/github/stars/aimod-cc/agent2api?style=flat&color=yellow)](https://github.com/aimod-cc/agent2api/stargazers)
[![platform](https://img.shields.io/badge/platform-Windows%20%7C%20Docker-blue)](https://github.com/aimod-cc/agent2api/releases/latest)

[✨ Key Features](#-key-features) • [💬 Community & Support](#-community--support) • [🤝 Supported Providers](#-supported-providers) • [🚀 Quick Start](#-quick-start) • [🐳 Docker Deployment](#-docker-deployment) • [📸 Screenshots](#-screenshots) • [🔧 Development & Build](#-development--build) • [📜 Usage Notice](#-usage-notice)

</div>

---

Agent2API brings the signed-in accounts of several AI desktop clients together behind one OpenAI-compatible endpoint. Point any client that accepts a custom `base_url` at `http://127.0.0.1:3065/v1` to call these accounts' model quota directly — **no upstream API key, no client changes**.

## ✨ Key Features

- 🔌 **OpenAI-compatible** — point your client at the gateway with a custom `base_url`; Chat, Responses and Messages protocols are all supported
- 🏪 **17 built-in providers** — WorkBuddy, Raccoon, CatPaw, Qoder, Trae and more work out of the box, plus custom providers via passthrough
- 🔁 **Credentials that renew themselves** — tokens refresh before they expire, near-expiry ones can be renewed in bulk, and the model catalog stays current
- 🪂 **Multi-account rotation** — accounts queue by priority, a 429 moves on to the next account, and rate-limited accounts are skipped instead of stalling
- 🛡️ **Egress proxies** — give each account its own exit IP; outbound fingerprints imitate the official clients
- 🔑 **Gateway security** — scoped gateway keys, human verification on login, and requests are refused by default until a key is configured
- 📊 **Request reporting** — request volume, success rate and token usage at a glance, plus cache hit rate and a 365-day heatmap
- ⏰ **Scheduled tasks** — auto check-in, credential maintenance and model catalog refresh, all managed on one page
- 💰 **Balance and usage** — balances shown right in the account rows, with concurrent queries across accounts
- 🖥️ **Two ways to run** — a tray-resident desktop app (no Node needed) or a Docker image with panel and gateway on one port, UI in six languages

## ❤️ Sponsors

<a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><img src="../assets/sponsor-proxylane.png" alt="ProxyLane (sponsor)" width="100%" /></a>

<table>

<tr>
<td width="240"><a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><picture><source media="(prefers-color-scheme: dark)" srcset="../assets/proxylane-logo-for-dark-bg.png" /><img src="../assets/proxylane-logo-for-light-bg.png" alt="ProxyLane" width="220" /></picture></a></td>
<td>Thanks to <a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme">ProxyLane</a> for sponsoring this project! ProxyLane provides native static residential ISP proxies for AI accounts (Static Residential ISP): IPs come from real home broadband operators with low Scamalytics fraud scores, each IP is dedicated to you, with unlimited traffic, from $3.90. Add one exit per account on the Network proxy page. Use code <code>AGENT2API25</code> for 25% off your first purchase.</td>
</tr>

</table>

## 💬 Community & Support

- **QQ group**: Agent2API chat group, group number `1127656296` (search the number in QQ to join); questions and feedback are welcome there.
- **💖 Donate**: if Agent2API helps you, you are welcome to [buy the author a coffee](./SPONSOR.md); donations are voluntary and unlock nothing.
- **🤝 Sponsorship / business**: want to sponsor the project, or feature your product here? Email [541675139@qq.com](mailto:541675139@qq.com).

## 🤝 Supported Providers

Reverse-proxy capabilities at a glance (✓ supported · ✗ not supported · — no such concept / not applicable):

| Platform | LLM requests | Token auto-refresh | Model list | Balance query | Check-in | Claims |
| --- | :--: | :--: | :--: | :--: | :--: | :--: |
| WorkBuddy (China) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| WorkBuddy (Global) | ✓ | ✓ | ✓ | ✓ | ✗ | — |
| Raccoon | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| CatPaw | ✓ | ✗ | ✓ | ✓ | ✗ | — |
| AutoClaw (domestic / international) | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| Qoder | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| Cline (Free / Pass) | ✓ | ✓ | ✓ | ✓ | — | — |
| Accio (international / domestic) | ✓ | ✓ | ✓ | ✓ | — | — |
| ZCode (domestic / international) | ✓ | ✗ | ✗ | ✓ | — | ✓ |
| CodeArts | ✓ | ✓ | ✓ | ✓ | — | ✓ |
| Trae | ✓ | ✓ | ✓ | ✓ | — | — |
| Loomy (iFlytek) | ✓ | ✗ | ✓ | ✓ | ✓ | — |
| KukuAI (Baidu Wenku) | ✓ | ✗ | ✓ | ✓ | ✓ | — |
| MonkeyCode (Chaitin, domestic / international) | ✓ | ✗ | ✓ | — | — | — |
| Command Code | ✓ | ✗ | ✓ | — | — | — |
| Antigravity (Google, Gemini) | ✓ | ✓ | ✓ | — | — | — |
| OfficeAce (Huawei Cloud, OfficeClaw) | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| Custom providers | ✓ | — | ✓ | — | — | — |

> Note: in Model list, ✓ means automatic online fetching is supported (some platforms also keep a built-in static table as a fallback); ZCode only has a built-in static table. ✗ in Token auto-refresh means the platform has no usable refresh flow (the account keeps working while its credentials are valid). Qoder check-in is available on the China version only.

`/v1/chat/completions`, `/v1/responses`, `/v1/messages` (incl. `count_tokens`) and `/v1/models` behave identically for every platform; model mapping, the global priority queue, 429 fallback, egress proxying, outbound fingerprint redaction and request reporting apply to all platforms alike.

> **This project is for learning and discussion only.** It reuses your own accounts' login state in the shape of a non-official client, which may violate the upstream services' terms; risks (including rate limiting or account bans) are your own. Commercial use and circumventing billing are prohibited. See the [Usage Notice](#-usage-notice) and [LICENSE](../LICENSE).
>
> This is a personal, local-purpose proxy tool, unaffiliated with any upstream vendor or their official products (the list is in the [Usage Notice](#-usage-notice)); interface shapes come from observing the clients' traffic, and upstream may change at any time.

## 🚀 Quick Start

Download the installer from [Releases](https://github.com/aimod-cc/agent2api/releases/latest) (NSIS, Simplified Chinese, installs to `C:\Program Files\Agent2API` by default, and needs administrator approval during setup), then launch it — **no Node or any other runtime required**.

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

## 🐳 Docker Deployment

```bash
docker run -d --name agent2api --restart unless-stopped \
  -p 3065:3065 -v ./data:/data \
  aimodcc/agent2api:latest
```

Open `http://<host>:3065` in a browser — the first visit walks you through **registering the admin account**; log in and create an API key in the "Gateway Keys" page for your clients — `http://<host>:3065/v1` is the OpenAI-compatible endpoint (it refuses to forward until the first key exists). All state (SQLite database / config / logs) lives in the `./data` volume.

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

Environment variables (all optional):

| Variable | Description |
| --- | --- |
| `AGENT2API_ADMIN_USER` + `AGENT2API_ADMIN_PASSWORD` | Preset the admin account & password (password in plain text, hashed automatically at startup). Leave unset to register in the panel |
| `AGENT2API_PANEL_PORT` | Serve the panel (UI + `/api/*`) on its own port; map only the main port publicly to keep the management plane internal (bind the panel port as `127.0.0.1:3066:3066`) |
| `AGENT2API_HOST` / `AGENT2API_PROXY_PORT` | Listen address (default `0.0.0.0`) / port (default `3065`) |
| `AGENT2API_ALLOW_NO_KEY` | Set to `1` to serve `/v1` without any key — private networks only |
| `AGENT2API_CAPTCHA_ENABLED` | Login-page human verification widget: `1` enabled (default), `0` disabled |
| `AGENT2API_SELF_UPDATE` | In-container update entry: `1` enabled by default (the panel offers "Download and update"); set `0` to upgrade manually only |
| `WORKBUDDY_UPDATE_REPO` + `WORKBUDDY_UPDATE_API_BASE` | Advanced: point update checks/downloads at your own source. The first swaps the repository (defaults to the upstream `aimod-cc/agent2api`; forks should override it), the second swaps the API root (default `https://api.github.com`, must return a GitHub-shaped release JSON). With a custom source set, the download allowlist additionally admits only **that source's own host** (scheme follows the source: an `http://` source also allows plaintext downloads — for local drills / internal mirrors only) |

Build from source: clone the repo and run `docker compose up -d --build` (the image contains only the gateway and the panel, no Rust toolchain).

**Web panel capability notes** (all stem from having no local desktop client): web login (WorkBuddy / Qoder / Cline), SMS codes and pasted credentials work fully; web logins that need a callback on this machine (AutoClaw / CatPaw / Accio / CodeArts / Trae) and "import desktop login state" are unavailable — use pasted credentials instead.

## 📸 Screenshots

### Accounts

![Accounts page: global queue, per-model rate-limit cooldown, expiry and balance](../assets/screenshots/accounts.png)

Add account:

![Add account: choose provider and edition, then sign in on the web](../assets/screenshots/add-account.png)

### Model management

![Model management: per-provider model catalog with rate, context and mappings](../assets/screenshots/models.png)

### Report

Overview: requests / success rate / tokens / top model, with account and provider rankings, usage donuts and a 365-day activity heatmap:

![Report overview: stat cards, top accounts / providers, model and provider usage donuts](../assets/screenshots/report-overview.png)

Trends: 24-hour **cache hit rate** (line) and **token consumption** (area) on twin axes, with a per-day token bar chart below:

![Report trends: dual-axis cache hit rate and token consumption, per-day token bars](../assets/screenshots/report-trends.png)

### Check-in center

![Check-in center: check-in overview and per-provider daily check-in groups](../assets/screenshots/checkin-center.png)

## 🔧 Development & Build

### Requirements

- Rust >= 1.77 and the Tauri 2 toolchain (to compile the desktop app itself; Windows also needs the WebView2 runtime)
- Node.js >= 18.17 (only to run `npm run tauri:*` and frontend build scripts; the desktop app does not depend on Node at runtime)

### Common scripts

```bash
npm run tauri:install      # Install desktop dependencies (same as npm --prefix desktop-tauri install)
npm run tauri:dev          # Launch the desktop app in dev mode (with hot reload)
npm run tauri:build        # Build the desktop installer

npm run build:icon         # Generate the icon source image (run after changing the icon design, then run tauri icon)
```

The build artifact is `target/release/bundle/nsis/Agent2API_<version>_x64-setup.exe` (currently about 3.0 MB; `src-tauri/.cargo/config.toml` points cargo's `target-dir` at the project root's `target/`).

## 📜 Usage Notice

### For learning and discussion only

This project is a hands-on exercise in HTTP reverse proxying, SSE streaming passthrough, multi-upstream protocol adaptation and desktop packaging (Tauri), and is **for personal learning and research only**. It is not an official product and has no affiliation with, endorsement from or sponsorship by Tencent and WorkBuddy / CodeBuddy, Meituan and CatPaw, SenseTime and Raccoon, Zhipu and AutoClaw / autoglm, Alibaba and Qoder / Accio, Huawei Cloud and CodeArts / OfficeAce, ByteDance and Trae, iFlytek and Loomy, Baidu Wenku and KukuAI, Chaitin and MonkeyCode, Command Code, or Google and Antigravity.

### About the reverse-proxy behaviour

What this project implements is a local reverse proxy: it reuses your own accounts' login state on your own machine and forwards requests to the official upstream gateways. It does not crack or bypass any payment or permission check — the quota it uses always comes from what your own account already has. That said, forwarding in the shape of a non-official client **may violate the upstream services' user agreements or terms of use**; whether to use it, and every consequence that follows (including but not limited to rate limiting, risk-control flags, account suspension or bans), is borne by the user.

### Prohibited uses

Do not use this project for any commercial purpose, for redistributing it for profit, for bulk account operation, for circumventing upstream billing or quota limits, or for any activity that violates local laws and regulations. To call these model services in production or commercial settings, use the official channels and official APIs.

### Credentials and data risk

This project stores account credentials (`accessToken` / `refreshToken`, etc.) as **plain text** in the local configuration directory (by default `~/.agent2api/`), and files produced by the export feature contain plain-text credentials as well. Keep them safe: never commit them to a public repository, upload them to cloud storage or share them with others. Losses caused by leaked credentials are borne by the user.

### No warranty and rights notice

This project is provided "as is"; the author makes no promise about its availability, stability, security or fitness for a particular purpose. Upstream interfaces may change at any time and the project may stop working and go unmaintained at any time. The full terms are in [LICENSE](../LICENSE). The interface shapes, protocol fields and other information in this project come from observing and organizing publicly visible network traffic of the official clients; all related trademarks and services belong to their respective owners. If a rights holder believes this project is inappropriate, please contact the author and it will be adjusted or removed promptly.

## 📄 License

This project is released under the [MIT License](../LICENSE); you may use, modify and distribute it freely as long as the copyright notice is retained.

One caveat: the LICENSE file carries a **Usage Notice** after the MIT text, whose clause 3 **adds restrictions on top of** MIT (no commercial use, no reselling redistributions, no bulk account operation). This project is therefore **not** pure MIT — **the MIT terms and the Usage Notice together form the complete license**, and where the two reach different conclusions on the same act, the stricter one governs. That is also why `Cargo.toml` points `license-file` at the LICENSE file instead of declaring the SPDX identifier `"MIT"`.

## ⭐ Star History

<a href="https://star-history.com/#aimod-cc/agent2api&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=aimod-cc/agent2api&type=Date&theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=aimod-cc/agent2api&type=Date" />
    <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=aimod-cc/agent2api&type=Date" />
  </picture>
</a>

<div align="center">

**[Download the latest release](https://github.com/aimod-cc/agent2api/releases/latest)** • **[Report an issue](https://github.com/aimod-cc/agent2api/issues)**

If this project helps you, a ⭐️ Star is appreciated!

</div>
