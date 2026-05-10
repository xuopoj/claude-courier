# Quickstart — consumer + proxy

You want **Claude Code working on this machine** (Linux, WSL, or a second Mac), with credentials synced from a Mac that's already logged in. Optionally, you also want to **route Claude API traffic through a proxy** for caching, logging, or to share one upstream key across local tools.

This guide assumes someone has already given you:

- a broker URL, e.g. `https://broker.example.com`
- a consume key, e.g. `c0a1b2c3...`
- (optional) an Anthropic API key, if you also want the proxy mode

You don't need to run a broker yourself. You don't need to build from source.

---

## 1. Install

One-liner. Auto-detects macOS arm64 / Linux x86_64 / Linux arm64:

```sh
curl -fsSL https://raw.githubusercontent.com/xuopoj/claude-courier/main/install.sh | sh
```

Verify:

```sh
claude-courier --version
```

---

## 2. Consume credentials from the broker

### Configure once

```sh
claude-courier consume-configure \
  --broker https://broker.example.com \
  --key <consume-key>
```

This writes a config file at `~/.config/claude-courier/consumer.toml` (or `~/Library/Application Support/claude-courier/consumer.toml` on macOS) with mode `0600`.

### Pull credentials

```sh
claude-courier consume
```

What this does:

1. GETs the latest published envelope from `<broker>/consume`.
2. Validates that `claudeAiOauth.accessToken` is present.
3. Atomically writes `~/.claude/.credentials.json` (mode `0600`).
4. Shallow-merges the identity fields (`userID`, `oauthAccount`, etc.) into `~/.claude.json`, preserving anything already there.

Test it works:

```sh
claude /status
```

Should report you as logged in.

### Keep credentials fresh

Claude OAuth access tokens are short-lived. Run `claude-courier consume` periodically so the token rotates here when the publisher publishes a new one. A systemd timer (Linux/WSL):

```ini
# /etc/systemd/system/claude-courier-consume.service
[Unit]
Description=claude-courier consume

[Service]
Type=oneshot
User=YOUR_USER
ExecStart=/usr/local/bin/claude-courier consume
```

```ini
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
journalctl -u claude-courier-consume --since '10 min ago'   # check it's firing
```

On macOS you'd use a launchd `.plist`; on cron-only systems, `*/15 * * * * /usr/local/bin/claude-courier consume` works.

#### Troubleshooting

| Symptom | Likely cause |
|---|---|
| `Error: HTTP 401 ...` | Wrong consume key. Re-check what you got from the broker operator. |
| `Error: HTTP 410 ... no token published yet` | Broker is up but the publisher hasn't published anything. Ask them to run `claude-courier publish` once. |
| `Error: credentials missing claudeAiOauth.accessToken` | The publisher's Keychain entry isn't a valid Claude Code credentials blob. Their problem to fix. |
| `claude /status` still says "not logged in" after a successful consume | Run `cat ~/.claude/.credentials.json` to confirm it has content; check `~/.claude.json` has `userID` and `oauthAccount`. |

---

## 3. Proxy mode (optional)

The proxy forwards HTTP requests to a target LLM API, optionally injecting an upstream API key and/or requiring a local auth key. Useful when you want one local endpoint that several tools can hit, or to apply rate limiting / logging in one place.

### Run with a built-in preset

```sh
claude-courier proxy --listen 127.0.0.1:8787 --upstream anthropic
```

Built-in presets:

| Preset | Forwards to |
|---|---|
| `anthropic` (or `claude`) | `https://api.anthropic.com` |
| `openai` | `https://api.openai.com` |
| `gemini` (or `google`) | `https://generativelanguage.googleapis.com` |

Anything else is treated as a literal URL — `--upstream https://api.example.com` works.

Test it:

```sh
curl -i http://127.0.0.1:8787/v1/messages \
  -H "x-api-key: $ANTHROPIC_API_KEY" \
  -H 'anthropic-version: 2023-06-01' \
  -H 'content-type: application/json' \
  -d '{"model":"claude-haiku-4-5-20251001","max_tokens":16,"messages":[{"role":"user","content":"hi"}]}'
```

Streaming (`stream: true`) works too — SSE is passed through unbuffered.

### Save defaults

```sh
claude-courier proxy-configure \
  --listen 127.0.0.1:8787 \
  --upstream anthropic
claude-courier proxy   # uses the saved config
```

### Inject an upstream key (so clients don't need it)

If you want clients to hit the proxy without sending their own key, configure the proxy to inject one:

```sh
claude-courier proxy-configure --inject-key sk-ant-...
claude-courier proxy
```

Now `curl http://127.0.0.1:8787/v1/messages -d '{...}'` works without the client sending `x-api-key`. The proxy attaches `x-api-key: sk-ant-...` for Anthropic upstreams, or `Authorization: Bearer ...` for OpenAI/Gemini. If the client *does* send a key, that key is forwarded instead — injection only fires when both `x-api-key` and `Authorization` are absent.

### Lock the proxy with a local auth key

If you bind to `0.0.0.0` (e.g. to share the proxy across a private LAN) you don't want anyone on the network using your upstream quota:

```sh
claude-courier proxy-configure \
  --listen 0.0.0.0:8787 \
  --upstream anthropic \
  --inject-key sk-ant-... \
  --listen-key my-local-shared-secret
claude-courier proxy
```

Now clients must send `x-api-key: my-local-shared-secret` to reach the proxy. The proxy strips that header before forwarding upstream — your local secret never leaks to Anthropic.

### Logging

Every request is logged with peer IP, method, path, status, and latency:

```
[2026-05-10 14:00:01] proxy listening on 127.0.0.1:8787 -> https://api.anthropic.com/
[2026-05-10 14:00:05] 127.0.0.1:54321 POST /v1/messages -> 200 (412 ms)
```

Body content is never logged.

---

## Other commands you probably won't need

```sh
claude-courier publish-configure ...   # only if you want to BE a publisher
claude-courier publish                  # only on a Mac that's logged into Claude Code
claude-courier broker-configure ...    # only if you're running your own broker
claude-courier broker                   # only if you're running your own broker
claude-courier --help
claude-courier <subcommand> --help
```

If you want to host your own broker too, see the main [README](README.md).
