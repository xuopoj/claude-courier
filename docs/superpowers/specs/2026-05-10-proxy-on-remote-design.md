# Deploy `proxy` subcommand on the remote server (phase 1)

Date: 2026-05-10
Status: approved for implementation

## Goal

Stand up the existing `proxy` subcommand of `claude-courier` on the same Lightsail box that already runs the broker, fronting `api.anthropic.com` at `https://proxy.aishipbox.com`. Phase 1 is a pure deploy + nginx job: no per-key issuance, no usage logging, no listen-side auth. Those land in phase 2.

## Non-goals (deferred to phase 2)

- Per-user / per-key issuance and revocation
- Usage logging (request count, tokens, timestamps, attribution)
- Persistence (SQLite or otherwise)
- Rate limiting
- Admin endpoints
- Listen-side auth in any form (`--listen-key`, header rename, etc.)

## Architecture

Run a second container `claude-courier-proxy` on the existing Lightsail host, from the same `ghcr.io/xuopoj/claude-courier:<tag>` image already used for the broker. It runs the `proxy` subcommand against the `anthropic` upstream preset, listens on `0.0.0.0:3008` inside the container, and is published to `127.0.0.1:3008` on the host. nginx terminates TLS on `proxy.aishipbox.com` and proxies to that loopback port.

```
client (sends Anthropic x-api-key)
   │ HTTPS
   ▼
nginx (proxy.aishipbox.com:443)
   │ proxy_buffering off, read_timeout 300s
   ▼
127.0.0.1:3008 (claude-courier-proxy container)
   │ no auth check, forwards as-is, streams response back
   ▼
api.anthropic.com
```

The broker container is untouched. The proxy container shares the host but is isolated by port (3007 vs 3008), container name (`claude-courier-broker` vs `claude-courier-proxy`), and nginx server block.

## Components touched

### 1. `src/proxy.rs`

Phase 1 is "deploy what we have," so behavior changes are minimal:

- The existing `listen_key` check in `handle()` still runs but is unreachable in production because `ProxyConfig::listen_key` will be `None` (no flag, no TOML on the server). The code stays as-is — removing it now adds churn for no win, and re-enabling auth in phase 2 is exactly what we want.
- Add a one-line comment near the `inject_api_key` branch in `forward()` warning that enabling key injection without listen-side auth turns the proxy into a free relay for the operator's Anthropic credits. This is the phase-2 footgun to prevent.

No other source changes in `src/`.

### 2. `.github/workflows/deploy.yml`

Extend the remote script in the `Roll container on server` step to start a second container after the broker is up:

```sh
echo "stopping previous proxy container (if any)"
docker rm -f claude-courier-proxy 2>/dev/null || true

echo "starting proxy container"
docker run -d \
  --name claude-courier-proxy \
  --restart unless-stopped \
  -p 127.0.0.1:3008:3008 \
  "$IMAGE_TAG" \
  proxy \
    --listen 0.0.0.0:3008 \
    --upstream anthropic
```

Mirror the existing broker readiness loop, hitting `http://127.0.0.1:3008/` and accepting any 4xx/2xx response (the proxy returns whatever Anthropic returns for `GET /` — likely 404 or 405 — both prove the proxy is serving). No new secrets are added in phase 1.

The existing `concurrency: deploy-broker` group still serializes deploys; rename to `deploy` in this change so the group name reflects what it now manages.

### 3. `deploy/proxy.aishipbox.com.nginx.conf` (new file)

Sibling to `broker.aishipbox.com.nginx.conf`. Port 80 only — TLS gets added by certbot in the post-deploy step. Differences from the broker config:

- `proxy_pass http://127.0.0.1:3008;`
- `proxy_buffering off;` — required so SSE streams flush byte-by-byte instead of waiting for nginx's buffer.
- `proxy_read_timeout 300s;` — Anthropic streams can run minutes long.
- `proxy_send_timeout 300s;` — symmetric.
- `client_max_body_size 10m;` — prompt payloads with attachments can exceed the 1m limit on the broker.
- File header comment documents the post-deploy steps (symlink, `nginx -t`, reload, certbot).

### 4. Manual post-deploy steps (one-time, documented in the nginx file's header)

1. `scp deploy/proxy.aishipbox.com.nginx.conf <user>@<host>:/tmp/`
2. SSH to host
3. `sudo mv /tmp/proxy.aishipbox.com.nginx.conf /etc/nginx/sites-available/proxy.aishipbox.com`
4. `sudo ln -s /etc/nginx/sites-available/proxy.aishipbox.com /etc/nginx/sites-enabled/`
5. `sudo nginx -t && sudo systemctl reload nginx`
6. Add an A record `proxy.aishipbox.com` → server IP (Route 53)
7. `sudo certbot --nginx -d proxy.aishipbox.com`

These are not part of CI in phase 1. They run once.

## Data flow

Request:
1. Client sends `POST https://proxy.aishipbox.com/v1/messages` with `x-api-key: <client's Anthropic key>`, `anthropic-version: 2023-06-01`, JSON body.
2. nginx terminates TLS, forwards to `127.0.0.1:3008` with `Host: proxy.aishipbox.com`, `X-Forwarded-For`, `X-Forwarded-Proto`.
3. Proxy receives the request, no listen-side check fires (`listen_key = None`), so it falls through to `forward()`.
4. `forward()` rewrites the URL to `https://api.anthropic.com/v1/messages`, strips hop-by-hop headers, sets `Host: api.anthropic.com`, leaves `x-api-key` intact, sends the body.
5. Anthropic responds with `text/event-stream`. The proxy streams via `bytes_stream() → Frame::data → StreamBody`.
6. nginx with `proxy_buffering off` flushes each frame to the client without delay.

## Error handling

| Failure | Result | Source |
|---|---|---|
| Client sends invalid Anthropic key | 401 from Anthropic, passed through | upstream |
| Client sends malformed JSON | 400 from Anthropic, passed through | upstream |
| Anthropic unreachable | 502 with "upstream error: ..." | `proxy.rs::handle` (existing) |
| Proxy container down | 504 from nginx | nginx default |
| Body too large (>10m) | 413 from nginx | `client_max_body_size` |
| Idle stream beyond 300s | 504 from nginx | `proxy_read_timeout` |

Nothing new to implement.

## Risk: open relay

The proxy is publicly reachable with no auth. Anyone with the URL can use it as egress to `api.anthropic.com`, but they must supply their own Anthropic key — the operator's account is **not** at risk because `inject_api_key` is unset.

Mitigation:
- The inline comment in `src/proxy.rs::forward()` warns future-self never to enable `inject_api_key` without first re-enabling listen-side auth.
- Phase 2 will add per-key auth, after which `inject_api_key` becomes safe to use.

## Testing

### Automated

None in phase 1. The proxy code path isn't modified beyond a comment, and the deploy workflow can't be unit-tested. The broker-config round-trip tests in `tests/config_test.rs` already cover TOML serde for `BrokerConfig`; `ProxyConfig` does not have a round-trip test today and we're not adding one until phase 2 changes the struct.

### Manual smoke (post-deploy)

```sh
curl -N https://proxy.aishipbox.com/v1/messages \
  -H "x-api-key: $ANTHROPIC_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -H "content-type: application/json" \
  -d '{"model":"claude-opus-4-7","max_tokens":64,"stream":true,"messages":[{"role":"user","content":"hi"}]}'
```

Expected: SSE frames arrive incrementally (not buffered into one chunk at the end), the response completes in seconds, and a non-streaming variant (drop `"stream":true`) returns valid JSON in a single response.

## Rollout

1. Land the code + deploy.yml + nginx file changes on `main` via PR.
2. Cut a tag (`git tag v0.x.y && git push --tags`). The existing tag-triggered workflow rolls both containers in one go.
3. Run the manual nginx + certbot steps once.
4. Run the smoke test. Done.

If the smoke test fails:
- `docker logs claude-courier-proxy` on the host
- nginx error log: `/var/log/nginx/error.log`
- DNS check: `dig proxy.aishipbox.com`
- Cert check: `sudo certbot certificates`

## Open questions

None — design approved by user on 2026-05-10.
