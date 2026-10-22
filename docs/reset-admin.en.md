# Reset the panel administrator (forgot password)

[简体中文](./reset-admin.md) | **English** | [繁體中文](./reset-admin.zh-Hant.md) | [日本語](./reset-admin.ja.md) | [한국어](./reset-admin.ko.md) | [Português (BR)](./reset-admin.pt-BR.md)

This page covers one situation: **you forgot the panel administrator password and cannot sign in**.

The password is stored only as a bcrypt hash, so it cannot be "recovered" — the only
way is to **delete** the administrator record, which returns the panel to the
"not registered" state so you can register a new one. This clears the administrator
account and password, but does not touch accounts, gateway keys, logs, or any other
business data.

> If you can still sign in, do not use this page: go to
> **Settings → Account → Change password**. That flow requires the old password,
> and revokes sign-ins on other devices automatically.

## 1. Stop the program

The record must be deleted while the program is **fully stopped**. The database runs
in WAL mode; writing while it is running may be overwritten or blocked by a lock.

| Form | How to stop |
| --- | --- |
| Windows desktop | Right-click the tray icon → **Exit** (closing the window is not enough: it minimizes to tray by default) |
| macOS / Linux desktop | Tray / menu bar icon → **Quit** (on macOS, `Cmd+Q` and confirm the tray icon disappears) |
| Windows server | Admin PowerShell: `Stop-Service Agent2API` |
| Linux server | `sudo systemctl stop agent2api` |
| Docker | In the compose directory: `docker compose stop` |

## 2. Locate the database file

The database is a single SQLite file, `agent2api.db`, inside the "config directory":

| Form | Path |
| --- | --- |
| Windows (desktop / server) | `%USERPROFILE%\.agent2api\agent2api.db` |
| macOS / Linux (desktop / server) | `~/.agent2api/agent2api.db` |
| Docker (compose default) | `./data/agent2api.db` on the host (`/data/agent2api.db` in the container) |

If `AGENT2API_PROXY_HOME` is set, the config directory is that value.

**Settings → Version info → Database file** shows the full path while you can still sign in.

## 3. Delete the administrator record

The administrator lives in the `kv` table under the key **`panelAdmin`**.

### With `sqlite3`

```bash
# Windows / macOS / Linux desktop and server
sqlite3 ~/.agent2api/agent2api.db "DELETE FROM kv WHERE key='panelAdmin';"

# Windows path form (PowerShell or CMD)
sqlite3 "%USERPROFILE%\.agent2api\agent2api.db" "DELETE FROM kv WHERE key='panelAdmin';"

# Docker: stop first, then delete from the host data directory
docker compose stop
sqlite3 ./data/agent2api.db "DELETE FROM kv WHERE key='panelAdmin';"
```

### Without `sqlite3`

Any SQLite GUI works (DB Browser for SQLite, DBeaver, Navicat, …): open
`agent2api.db` → open the `kv` table → delete the row whose `key` is `panelAdmin` →
save / write changes.

Python also works (usually preinstalled):

```bash
python -c "import sqlite3,os; p=os.path.expanduser('~/.agent2api/agent2api.db'); c=sqlite3.connect(p); c.execute(\"DELETE FROM kv WHERE key='panelAdmin'\"); c.commit(); c.close(); print('done')"
```

## 4. Restart and register again

Restart the program (`docker compose start` for Docker) and open the panel:

- **Desktop**: the window goes straight to the main UI (no administrator = no login).
  To set a password again, open **Settings → Account** — it will point you to the
  login page for registration; you can also open `http://127.0.0.1:3065/login` directly.
- **Server / Docker**: opening the panel redirects to the login page, which shows
  **"First use: create an administrator account"**. Fill in a username and password.

After registering, the panel requires sign-in again.

## 5. Note: environment variables only apply on first run

If your deployment sets:

```bash
AGENT2API_ADMIN_USER=admin
AGENT2API_ADMIN_PASSWORD=your-password
```

they are used **only for the initial seed** — written once when the database has no
administrator, after which the database is authoritative (that is what makes
changing the password in the panel possible). Therefore:

- Changing these variables does **not** change an already registered password;
- After deleting `panelAdmin` and restarting, if the variables are still present they
  **seed a new administrator** — in that case the "new password" is the one in the
  variables.

Remove the variables from your deployment config before restarting if you do not
want it to recreate the account automatically.

`AGENT2API_ADMIN_PASSWORD_HASH` (a bcrypt hash, takes precedence over the plaintext
password) behaves the same way.

## 6. Security note

After deleting the administrator record, the panel is **unauthenticated** in the
"not registered" state:

- Desktop: only reachable from this machine, so risk is limited — but if
  "allow LAN access" is enabled, other devices on the LAN can open the panel too.
  Register again promptly.
- Server / Docker: **anyone who can reach the panel port can take over the management
  surface**. Restart and register immediately; do not linger in between. This matters
  most when the management port is exposed to the internet.

If you accidentally damage the database, stop: there may be a `backups/` directory
next to it (safety snapshots taken before each full-database restore) — copy a
`pre-restore-*.db` back over the file to recover.
