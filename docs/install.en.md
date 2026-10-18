# Installation & Deployment

This page is the complete install-and-operate guide for all three delivery forms: **desktop** (Windows / macOS / Linux), **server** (Windows / Linux, headless) and **Docker**. If you are not sure which one to pick, start with the table below.

Every form is managed from the **browser panel** afterwards (accounts, gateway keys, reports) — the panel shares the gateway's port; the desktop form additionally has an app window.

[简体中文](./install.md) | **English** | [繁體中文](./install.zh-Hant.md) | [日本語](./install.ja.md) | [한국어](./install.ko.md) | [Português (BR)](./install.pt-BR.md)

## Contents

- [Which form to pick](#which-form-to-pick)
- [Desktop (Windows / macOS / Linux)](#desktop-windows--macos--linux)
- [Server (Windows / Linux, headless)](#server-windows--linux-headless)
- [Docker](#docker)
- [Verification and client setup](#verification-and-client-setup)
- [LAN access](#lan-access)
- [Mobile](#mobile)

## Which form to pick

Releases ship the artifacts below; pick the row that matches your machine:

| Form | Artifact | Interface | Notes |
| --- | --- | --- | --- |
| **Windows desktop** | `Agent2API_<version>_x64-setup.exe` | App window | NSIS installer, installs to `C:\\Program Files\Agent2API` by default, needs administrator approval |
| **macOS desktop** | `Agent2API_<version>_universal.dmg` | App window | Universal binary (Apple Silicon and Intel each built, then merged); macOS 10.15+ |
| **Linux desktop** | `Agent2API_<version>_amd64.deb` or `Agent2API_<version>_amd64.AppImage` | App window | deb installs system-wide (needs webkit2gtk-4.1 / gtk3); the AppImage is a single self-contained file that runs on any distro |
| **Windows server** (no GUI) | `agent2api-server-windows-amd64.zip` | **Browser panel** | Command-line install, registered as a Windows service — see [Server](#server-windows--linux-headless) |
| **Linux server** (no GUI) | `agent2api-server-linux-<arch>.tar.gz` | **Browser panel** | Command-line install, run under systemd; the same package is also the in-container update payload |
| **Docker** | `aimodcc/agent2api:latest` | **Browser panel** | amd64 / arm64 images — see [Docker](#docker) |

> Which one: **you have a desktop** (want the tray, launch-at-login, local login-state import) → the desktop app;
> **the server has no graphical interface** (Windows Server / a Linux box) → the server package;
> **you already run Docker** → the image.
>
> The desktop form needs a graphical session and **cannot be installed on a headless server**.
> The server package links no WebView / GTK at all: one executable plus a `ui/` directory is everything it needs.

## Desktop (Windows / macOS / Linux)

**Desktop apps** (Windows / macOS / Linux) simply launch after installation — **no Node or any other runtime required**; the gateway runs inside the app process and stays in the tray.

1. First launch starts the local gateway (port 3065) inside the app process and opens the main window. If an older version's data directory or data files are found, a dialog walks you through the migration.
2. Click "Add account" on the Accounts page, pick a provider and follow the dialog: sign in or fill in credentials (web login / SMS code / pasted credentials / importing this machine's desktop login state).
3. Set your OpenAI client's `base_url` to `http://127.0.0.1:3065/v1` and put anything in `api_key` (for example `sk-local`; the server does not check it while authentication is disabled).

Closing the window minimizes to the tray by default and the gateway keeps forwarding in the background; to quit for real, right-click the tray icon and choose "Exit".

> **First launch on macOS**: the app inside the dmg is neither code-signed nor notarized (signing needs a paid Apple Developer account — see `.github/workflows/build.yml`), so Gatekeeper blocks the first launch. Allow it once under "System Settings → Privacy & Security" and it opens normally afterwards.
>
> **Linux desktop notes**: the tray needs `libayatana-appindicator3` (most desktops ship it; install `libayatana-appindicator3-1` if the tray icon does not appear, then restart). An AppImage needs the executable bit before its first run (`chmod +x`; the app sets it automatically when you pick "Open installer"). Updates: deb goes through the system installer (it asks for your password once), while an AppImage is upgraded by replacing the file.
>
> **Desktop differences**: "import desktop login state" for AutoClaw and ChatGLM (智谱清言) is Windows-only (it reads Windows DPAPI ciphertext, so that section is hidden on macOS / Linux); the others (Raccoon / CatPaw / Cline) read plaintext files under `HOME` and work on all three platforms. Tray, launch-at-login, LAN access and web-panel hosting are supported on macOS / Linux too.

## Server (Windows / Linux, headless)

For **servers without a graphical interface** (Windows Server, Linux boxes, cloud instances). The artifacts are one and the same `agent2api-server` binary: no window, no tray, no WebView / GTK linkage, and no Node or any other runtime at run time. The command only **installs**; everything after that (accounts, keys, reports) happens in the browser panel.

The service runs as a system service: launched at boot, restarted automatically after a crash, no console in the background.

### Windows Server

Download `agent2api-server-windows-amd64.zip`, unpack it and open PowerShell **as administrator**:

```powershell
Expand-Archive agent2api-server-windows-amd64.zip -DestinationPath C:\agent2api-pkg
cd C:\agent2api-pkg
.\install-service.ps1
```

The script installs the program to `C:\Agent2API`, puts data in `C:\ProgramData\Agent2API`, registers a Windows service named `Agent2API` (`LocalSystem`, starts at boot), adds a firewall inbound rule, then starts it.

```powershell
# Custom location and port
.\install-service.ps1 -InstallDir D:\apps\agent2api -DataDir D:\data\agent2api -Port 8080

# Day-to-day management
Get-Service Agent2API
Restart-Service Agent2API
Stop-Service Agent2API

# Uninstall (keeps data); add -Purge to delete program and data together
.\install-service.ps1 -Uninstall
```

**Upgrading**: unpack the new version and run `install-service.ps1` again (it stops the service, overwrites program and UI, then starts it again; data and config are untouched).

> **Two Windows-specific traps that the script already handles**:
> a service process's working directory is `C:\Windows\system32`, not the folder the exe lives in — the UI directory is located from the exe path, so it is unaffected; the data directory is written explicitly into the service's registry entry as `AGENT2API_PROXY_HOME` (without it, data would land under LocalSystem's `C:\Windows\system32\config\systemprofile`, where nobody would ever find it).
> A service has no console, so logs also go to the file `C:\ProgramData\Agent2API\logs\server.log`.

### Linux (systemd)

```bash
tar -xzf agent2api-server-linux-amd64.tar.gz
cd agent2api-server-linux-amd64      # or whatever the unpacked directory is called
sudo bash install.sh
```

The script creates an `agent2api` system account (login disabled), installs the program to `/opt/agent2api`, puts data in `/var/lib/agent2api`, writes config to `/etc/agent2api/agent2api.env`, and installs the systemd unit with `enable + start`.

```bash
# Upgrade (keeps data and config; the old binary is backed up as .bak)
sudo bash install.sh --upgrade

# Uninstall (keeps data); add --purge to delete data and config together
sudo bash install.sh --uninstall

# Day-to-day management
systemctl status agent2api
systemctl restart agent2api
journalctl -u agent2api -f
```

Launch at boot, automatic restart after a crash (`Restart=always`) and graceful shutdown on SIGTERM (it waits for in-flight requests, so long-lived SSE connections are not cut off) are all preconfigured. After editing `/etc/agent2api/agent2api.env`, run `systemctl restart agent2api` to apply it.

> **Do not enable `AGENT2API_SELF_UPDATE` on bare metal**: that is the in-container mechanism (replace files, exit, let the container's restart policy bring it back). On a bare-metal host the program files belong to root and the service account cannot replace them; the unit sets it to `0` explicitly. Upgrade with `install.sh --upgrade` instead.

### After installing

Both platforms are the same — open a browser:

```
http://<server-ip>:3065/
```

1. The first visit walks you through **registering the admin account** (it can also be preset with an environment variable, see below)
2. Once logged in, create an API key on the "Gateway Keys" page
3. Point your client's `base_url` at `http://<server-ip>:3065/v1` and use the key you just created as `api_key`

> ⚠️ **`/v1/*` refuses service until a key exists** (fail-closed). That is deliberate: the server form binds `0.0.0.0`, and exposing upstream quota to the whole network without a key is not acceptable.

### Unattended deployment

Both the administrator and the gateway key can be preset with environment variables, skipping the manual browser steps:

| Variable | Description |
| --- | --- |
| `AGENT2API_ADMIN_USER` + `AGENT2API_ADMIN_PASSWORD` | Preset the administrator (password in plain text, converted to bcrypt at startup); `AGENT2API_ADMIN_PASSWORD_HASH` takes a ready-made hash instead |
| `AGENT2API_PROXY_API_KEY` | Preset one gateway key, skipping "create the first one in the panel" |
| `AGENT2API_PROXY_HOME` | Data directory |
| `AGENT2API_HOST` / `AGENT2API_PROXY_PORT` | Listen address (default `0.0.0.0`) / port (default `3065`) |
| `AGENT2API_PANEL_PORT` | Split the panel onto its own port: the management plane listens there and the main port only serves `/v1/*` |
| `AGENT2API_ALLOW_NO_KEY` | Set to `1` to turn fail-closed off (private networks only) |
| `AGENT2API_LOG_FILE` | Log file path, default `{data directory}/logs/server.log` |
| `AGENT2API_VERBOSE` | Set to `1` for debug logging |

On Linux put them in `/etc/agent2api/agent2api.env`; on Windows edit the service registry key `HKLM\SYSTEM\CurrentControlSet\Services\Agent2API\Environment` (`install-service.ps1` already writes the basics there; rerunning the script overwrites it).

> **Accounts cannot be preset**: they need a QR scan / SMS code / pasted credentials, which the upstream protocols decide, so there is no way around it. For bulk deployment, copy a fully configured **data directory** over (the SQLite database and `account.key` live side by side), and the accounts move with it.

## Docker

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
| `WORKBUDDY_UPDATE_REPO` + `WORKBUDDY_UPDATE_API_BASE` | Advanced: point update checks/downloads at your own source. The first swaps the repository (defaults to the upstream `Agent2API/agent2api`; forks should override it), the second swaps the API root (default `https://api.github.com`, must return a GitHub-shaped release JSON). With a custom source set, the download allowlist additionally admits only **that source's own host** (scheme follows the source: an `http://` source also allows plaintext downloads — for local drills / internal mirrors only) |

Build from source: clone the repo and run `docker compose up -d --build` (the image contains only the gateway and the panel, no Rust toolchain).

**Web panel capability notes** (these stem from having no local desktop client): web login (WorkBuddy / Qoder / Cline), SMS codes and pasted credentials work fully; web logins that need a callback on this machine (AutoClaw / CatPaw / Accio / CodeArts / Trae) and "import desktop login state" are unavailable — use pasted credentials instead. A desktop app with "expose the web management panel" turned on shows the same differences in a browser: one interface, one set of rules.

### In-container updates (optional)

The panel's "Software update" page can download and apply updates from inside the container: it fetches `agent2api-server-linux-<arch>.tar.gz` from the official Release (verifying the `.sha256`), replaces the executable and `ui/`, then exits gracefully so the `restart` policy brings the same container back up. Controlled by `AGENT2API_SELF_UPDATE` (set to `1` in the Dockerfile); set it to `0` to upgrade manually only.

The same `agent2api-server-linux-<arch>.tar.gz` is also the installer package for the [Linux server](#linux-systemd): its `agent2api-server` and `ui/` entries are exactly what the update chain reads by name, and it additionally ships `install.sh` / `agent2api.service` / `agent2api.env.example`. The update chain only reads the first two (the rest are discarded — see `selfupdate::extract_bundle`), so one artifact serves both purposes: unpack and replace inside a container, unpack and run `install.sh` on a bare-metal host.

Be aware of the trade-off: after an in-container update the image tag and the version actually running **drift apart** (`docker inspect` still reports the old tag), and `docker compose pull && up -d` rebuilds the container back to the version baked into the image. If you want pinned versions, leave this entry point off.

## Verification and client setup

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

## LAN access

By default the gateway only listens on `127.0.0.1`. After turning on "Settings → General → LAN access" it listens on all adapters, and devices on the LAN just point their API base URL at this machine's IP (the UI shows the full address) to share the accounts. For safety, enabling it requires registering a panel administrator first: the management API then demands an admin session or a gateway key, and with no key enabled the forwarding API refuses service too (the enable flow adds a "default" key automatically). The web panel can optionally be exposed to the LAN as well (off by default). Changes take effect after a restart.

The desktop installers (Windows / macOS) already ship the web panel's `ui/` folder: once you turn on "expose the web management panel to the LAN" as well, devices on the LAN can open the same address in a browser to get the management UI — the same interface as the Docker form. **The desktop window itself never goes through web login** (there is no login page in the app): it authenticates with this machine's first gateway key, so flipping that switch never asks you to sign in from the desktop app. The web login page only appears when a browser opens the panel.

## Mobile

There are two forms — pick what you need:

| Form | How to install | Good for |
| --- | --- | --- |
| **Browser / PWA** | Open `/m` in a phone browser; optionally "Add to Home Screen" | Quick look, nothing to install |
| **Android APK** | Install `Agent2API-<version>.apk` | Home-screen icon, full screen, multiple servers |

Both forms run the same UI code. The difference is who serves the page: the PWA is served by the gateway (so "also expose the web management panel" must be on), while the APK ships the UI inside the package (so it can start the UI offline and does not need that switch).

### Prerequisites (by form)

**Desktop (Windows / macOS / Linux)**: both switches, one without the other will not work.

1. "Settings → General → LAN access" — lets devices on the LAN reach the gateway. The enable flow asks you to register a panel administrator first and adds a key named "default" automatically.
2. "Settings → General → LAN access → also expose the web management panel" — serves the `ui/` static folder. **Required for the browser / PWA form** (`/m` is served by the gateway, so it returns 404 while this switch is off). **The APK does not need it** (the UI is inside the package), but it still needs switch 1 — otherwise the gateway only listens on `127.0.0.1` and the phone cannot reach it.

Both require a restart to take effect.

**Server / Docker**: no extra switches — both forms already listen on all adapters and serve `ui/` by default (`AGENT2API_UI_DIR=/app/ui` is hard-coded in the Dockerfile). You only need the container's or service's port to be reachable from the LAN. If you manually set `AGENT2API_HOST=127.0.0.1`, change it to `0.0.0.0`; if `AGENT2API_UI_DIR` points to a missing directory the panel returns 404 while the API keeps working.

### Open it on the phone (browser / PWA)

`http://<LAN IP of the machine running the gateway>:3065/m`

On desktop, that is the address shown in the settings pane above (for example `192.168.1.23`) — replace the trailing `/v1` with `/m`. For Docker / server use the host machine's LAN IP and the mapped port.

The first visit redirects to the login page; sign in with the panel administrator and it returns you to the mobile UI automatically. If no administrator exists yet, the login page asks you to create one.

### Install the APK (Android)

On first launch you get a "Servers" screen. Three steps:

1. Tap "Add" and enter the gateway's `IP:port` (e.g. `192.168.1.23:3065`; the port defaults to 3065);
2. Tap "Connect & save" — it probes the gateway first, so a wrong address or a gateway without LAN access is reported right there;
3. A login page appears. Enter this gateway's administrator account and password (if none exists yet, it becomes "Create admin account").

**Multiple servers**: tap "Add" again. Each server keeps its own sign-in — switching over does not require signing in again, and neither does switching back. The entry point is "Settings → Servers"; tap a row to switch to it.

**Known limitations of the APK**:

- Android only. iOS requires an Apple Developer account for signing, or installing via Xcode yourself (sideloaded signatures expire after 7 days).
- It talks to `http://` LAN addresses in cleartext. The risk is "sniffable on the same LAN"; the gateway itself still requires administrator login. To remove it, give the gateway TLS and enter an `https://` address (the parsing is the same, it just will not prepend `http`).
- The UI follows the APK version, not the gateway. If a gateway upgrade adds new screens, you need a newer APK to see them; pure data changes are unaffected.

### Add to home screen (browser / PWA)

Optional — gives you full screen with no address bar. The mobile UI offers the entry point under "Settings → Install as app":

- **Android / Chrome**: tap "Add to Home Screen" in settings (opens the system install dialog); or use the menu → "Add to Home screen" / "Install app"
- **iOS / Safari**: Safari has no install API, so the settings page spells out the steps — Share → "Add to Home Screen"

Opening it from the home screen icon lands directly on the mobile home page. When already running in a standalone window, the settings page shows "Installed".

When a new version is available, a "Restart & update" bar floats at the bottom: after a gateway upgrade the new UI is already downloaded, and one tap switches to it (ignoring it is fine too — it takes effect on the next open).

> **Why no native app or store package**: the mobile UI always talks to a gateway running on **another machine** (the gateway needs to read that machine's client logins and write its config files — it cannot run on a phone). It is fundamentally "remote access to an HTTP service", and the browser is the right carrier: same-origin auth, no install, and it upgrades together with the gateway. The PWA form adds full screen and a home-screen icon, so it feels like a native app.
>
> **Service Worker (offline shell) only works over `https` or `localhost`**: LAN access uses `http://<IP>`, where browsers do not provide SW, so the mobile UI will not open while offline. That is a browser security restriction, not a missing feature — in normal use the gateway is online, so it does not matter.

### What it can and cannot do

| Can | Cannot (use the desktop app) |
| --- | --- |
| Reports, account status, balances, request logs, runtime logs | Add accounts (QR / paste token / import client login) |
| Enable/disable accounts, refresh credentials, delete accounts | Edit an account's credentials, proxy or rate-limit rules |
| One-tap check-in, view check-in history | Claim onboarding rewards and plans one by one |
| Enable/disable models, filter by provider | Edit model mappings, reasoning levels, capability flags |
| Copy gateway keys, enable/disable keys | Create keys, set provider/model allowlists |
| Enable/disable proxies, test connectivity | Add proxies, manage subscriptions, download the kernel |
| Enable/disable scheduled tasks, change intervals | Change port, routing, retry, timeouts, data retention |
| Assistant quick queries (balances / duplicate models / today's usage / rate-limited accounts) | Free-form assistant chat and tool calling |
| View Agent takeover status, restore | Rewrite client configs (a phone cannot read local files) |

The desktop sidebar has an "Open on phone" entry at the bottom that leads to the same page.
