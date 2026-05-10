# claude-courier

Sync Claude Code OAuth credentials between machines through a self-hosted broker.

A single binary with four roles:

- **publisher** — reads Claude credentials from macOS Keychain on a logged-in Mac and publishes them to a broker.
- **broker** — a small HTTP server that holds the latest published envelope.
- **consumer** — pulls the envelope from the broker and applies it to `~/.claude/.credentials.json` + `~/.claude.json` on Linux/WSL/another Mac.
- **proxy** — an unrelated bonus: an HTTP reverse proxy with built-in presets for `anthropic`, `openai`, and `gemini`. Useful for routing local LLM clients through a single endpoint.

Why exist: Claude Code's login flow ties your tokens to one machine's Keychain. If you also want Claude Code working on a Linux server, WSL box, or second Mac, you'd normally re-authenticate there. claude-courier syncs the tokens once and keeps the secondary machines current as your access token rotates.

> **Just want to consume from someone else's broker (and/or use the proxy)?** Skip ahead to [QUICKSTART.md](QUICKSTART.md). The rest of this README is for people running their own broker.

## Install

### Prebuilt binary (macOS arm64, Linux x86_64, Linux arm64)

```sh
curl -fsSL https://raw.githubusercontent.com/xuopoj/claude-courier/main/install.sh | sh
```

Environment overrides for the installer:

- `VERSION=v0.1.0` — install a specific tag (default: latest release)
- `PREFIX=/usr/local/bin` — install dir (default: `/usr/local/bin`, falls back to `~/.local/bin` without sudo)
- `NO_SUDO=1` — skip sudo, install to `~/.local/bin`

### From source

```sh
git clone https://github.com/xuopoj/claude-courier
cd claude-courier
cargo install --path .
```

## Token sync — quick start

### 1. Run the broker on a server you control

The broker is a Docker image at `ghcr.io/xuopoj/claude-courier:latest`. Run it behind a TLS reverse proxy (nginx + Let's Encrypt is fine — see [`deploy/`](deploy/)):

```sh
docker run -d \
  --name claude-courier-broker \
  --restart unless-stopped \
  -p 127.0.0.1:3007:3007 \
  ghcr.io/xuopoj/claude-courier:latest \
  broker \
    --bind 0.0.0.0:3007 \
    --publish-key "$(openssl rand -hex 32)" \
    --consume-key "$(openssl rand -hex 32)"
```

Save the two keys. The publisher needs `--publish-key`; consumers need `--consume-key`. Different keys so a leaked consumer machine can't push poisoned credentials back.

### 2. Configure the publisher (the Mac that's logged into Claude Code)

```sh
claude-courier publish-configure \
  --broker https://broker.example.com \
  --key <publish-key>
```

Then publish whenever your token rotates (or on a schedule):

```sh
claude-courier publish
```

This:

1. Shells out to `security find-generic-password -s "Claude Code-credentials" -w` to read the credentials JSON from macOS Keychain.
2. Reads `userID`, `oauthAccount`, `firstStartTime`, `hasCompletedOnboarding` from `~/.claude.json` (the minimum identity blob a fresh Claude Code install needs to recognize you as logged in).
3. Validates `claudeAiOauth.accessToken` is present.
4. POSTs the bundle as `{credentials, identity}` to `<broker>/publish` with `x-api-key: <publish-key>`.

### 3. Configure consumers (Linux server, WSL, another Mac)

```sh
claude-courier consume-configure \
  --broker https://broker.example.com \
  --key <consume-key>

claude-courier consume
```

The consumer:

1. GETs `<broker>/consume` with `x-api-key: <consume-key>`.
2. Parses the envelope, validates `credentials.claudeAiOauth.accessToken`.
3. Atomically writes `~/.claude/.credentials.json` (mode `0o600`).
4. Shallow-merges the identity fields into `~/.claude.json`, preserving any pre-existing fields.

After the first consume, `claude /status` should show you logged in.

#### Keep consumers fresh on a schedule

Claude OAuth access tokens are short-lived. To keep a consumer in sync, run `claude-courier consume` periodically:

```sh
# /etc/systemd/system/claude-courier-consume.service
[Unit]
Description=claude-courier consume

[Service]
Type=oneshot
User=youruser
ExecStart=/usr/local/bin/claude-courier consume
```

```sh
# /etc/systemd/system/claude-courier-consume.timer
[Unit]
Description=claude-courier consume every 15 min

[Timer]
OnBootSec=2min
OnUnitActiveSec=15min
Persistent=true

[Install]
WantedBy=timers.target
```

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now claude-courier-consume.timer
```

## Broker endpoints

| Method | Path | Auth | Behavior |
|---|---|---|---|
| `POST` | `/publish` | `x-api-key: <publish-key>` | Replaces the single slot. Body is opaque bytes (the envelope JSON). Returns `202 Accepted`. |
| `GET` | `/consume` | `x-api-key: <consume-key>` | Returns the slot if filled, `410 Gone` if no token has been published yet. **Does not clear the slot** — multiple consumers can each fetch the latest. |
| anything else | | | `404 not found` |

Storage is in-memory only. A broker restart drops the slot and you'll need to re-publish.

## Reverse proxy mode (bonus)

```sh
claude-courier proxy --listen 127.0.0.1:8787 --upstream anthropic
claude-courier proxy --listen 127.0.0.1:8080 --upstream openai
claude-courier proxy --listen 127.0.0.1:8080 --upstream https://api.example.com
```

Built-in presets: `anthropic` → `https://api.anthropic.com`, `openai` → `https://api.openai.com`, `gemini` → `https://generativelanguage.googleapis.com`. Anything else is treated as a literal URL.

Features:

- Streaming response passthrough (SSE works).
- Optional listen-side `x-api-key` gate (`proxy-configure --listen-key ...`) so you can bind to `0.0.0.0` without leaving the upstream open.
- Optional upstream API-key injection (`proxy-configure --inject-key ...`) — picks `x-api-key` for Anthropic, `Authorization: Bearer` for everything else, and only injects if the client didn't already send one.
- Hop-by-hop header stripping; doesn't leak the listen-side key to the upstream.
- Request/response logging with status + latency.

## Security notes

- **The broker holds OAuth tokens.** Run it behind TLS with a real cert. The Dockerfile binds the broker to `127.0.0.1:3007` inside the container, expecting a reverse proxy (nginx/Caddy) to terminate TLS in front of it.
- **Use separate publish/consume keys.** A consumer machine should not be able to overwrite the slot.
- **Keys are 32-byte hex (`openssl rand -hex 32`)** in the suggested setup, compared with constant-time-ish equality. The current implementation uses byte equality which leaks timing information; for a personal-use broker on a fast network this is fine, but a treasure-trove deployment should switch to a constant-time compare.
- **Storage is in-memory.** Restarting the broker means the latest envelope is gone until the publisher re-publishes. Acceptable for a token-sync use case.
- **The Docker image contains no secrets.** Keys are passed at `docker run` time via `--publish-key` / `--consume-key`. The image can stay public.

## Project layout

```
src/
  main.rs       # clap CLI: publish, consume, broker, proxy + their *-configure subcommands
  config.rs     # PublisherConfig / BrokerConfig / ConsumerConfig / ProxyConfig + load/save/resolve
  client.rs     # publish() and consume() — Keychain read, identity merge, atomic write
  broker.rs     # hyper server, single-slot Mutex<Option<Bytes>>
  proxy.rs      # hyper server, reqwest streaming passthrough
  http.rs       # auth_get / auth_post_json helpers (used by client.rs)
  log.rs        # timestamped eprintln
.github/workflows/
  deploy.yml    # tag push -> build image, push to GHCR, SSH to server, roll container
  release.yml   # tag push -> matrix-build 3 platforms, attach tarballs to GitHub Release
deploy/
  broker.aishipbox.com.nginx.conf   # example nginx site config
install.sh      # POSIX-sh installer that picks the right release tarball
Dockerfile      # multi-stage rust:bookworm builder + debian-slim runtime
```

## Releasing

Cutting a release deploys the broker AND publishes binaries:

```sh
git tag v0.1.1
git push --tags
```

Two workflows fire in parallel:

- `release.yml` builds binaries for `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` and attaches them as a GitHub Release.
- `deploy.yml` builds the Docker image tagged `:v0.1.1` + `:latest`, pushes to GHCR, SSHes to the server defined in repo secrets, and rolls the broker container.

Routine commits to `main` do not deploy — only tags.

## License

MIT
