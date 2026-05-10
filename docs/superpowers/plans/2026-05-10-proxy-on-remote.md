# Proxy on Remote Server (Phase 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deploy the existing `proxy` subcommand alongside the broker on the Lightsail box, fronting `api.anthropic.com` at `https://proxy.aishipbox.com`. No listen-side auth, no usage logging — phase 1 is deploy-only.

**Architecture:** A second container `claude-courier-proxy` runs from the same image as the broker, listens on host loopback `127.0.0.1:3008`, and is fronted by a new nginx site `proxy.aishipbox.com`. Tag-triggered deploy.yml is extended to roll both containers; nginx site config + cert are a one-time manual step.

**Tech Stack:** Rust 2024, Docker, GitHub Actions, nginx, certbot, hyper/reqwest (existing).

**Spec:** `docs/superpowers/specs/2026-05-10-proxy-on-remote-design.md`

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `src/proxy.rs` | Modify (lines 189-200) | Add a footgun-prevention comment near the `inject_api_key` branch |
| `.github/workflows/deploy.yml` | Modify (lines 28-32, 100-156) | Rename concurrency group; add second `docker run` for proxy + readiness probe |
| `deploy/proxy.aishipbox.com.nginx.conf` | Create | nginx site config for the proxy hostname |

No source files beyond `src/proxy.rs` change. No tests change (the proxy code path is not modified beyond a comment, and the deploy workflow is not unit-testable).

---

## Task 1: Add inject-api-key footgun comment

**Files:**
- Modify: `src/proxy.rs:189-200`

- [ ] **Step 1: Read the current code**

Open `src/proxy.rs` and confirm lines 189-200 currently read:

```rust
    if let Some(key) = &state.inject_api_key {
        if !forwarded_auth && !forwarded_xkey {
            // Anthropic uses x-api-key; OpenAI/Gemini use Authorization. Default
            // to x-api-key when upstream is anthropic, else Authorization Bearer.
            if host_str.contains("anthropic") {
                upstream_req = upstream_req.header("x-api-key", key);
            } else {
                upstream_req =
                    upstream_req.header("authorization", format!("Bearer {key}"));
            }
        }
    }
```

If the line numbers have shifted, locate the `if let Some(key) = &state.inject_api_key {` block — that's the target.

- [ ] **Step 2: Insert the warning comment**

Use Edit to replace:

```rust
    if let Some(key) = &state.inject_api_key {
        if !forwarded_auth && !forwarded_xkey {
            // Anthropic uses x-api-key; OpenAI/Gemini use Authorization. Default
            // to x-api-key when upstream is anthropic, else Authorization Bearer.
```

with:

```rust
    if let Some(key) = &state.inject_api_key {
        // SAFETY: never enable inject_api_key without listen_key. An open relay
        // that injects the operator's key lets anyone drain the operator's
        // upstream credits.
        if !forwarded_auth && !forwarded_xkey {
            // Anthropic uses x-api-key; OpenAI/Gemini use Authorization. Default
            // to x-api-key when upstream is anthropic, else Authorization Bearer.
```

- [ ] **Step 3: Verify the build still succeeds**

Run: `cargo build`
Expected: builds clean, no warnings about the comment.

- [ ] **Step 4: Run the tests**

Run: `cargo test`
Expected: all 3 existing tests pass (`broker_config_roundtrip` and friends).

- [ ] **Step 5: Commit**

```sh
git add src/proxy.rs
git commit -m "$(cat <<'EOF'
proxy: warn against enabling inject_api_key without listen_key

Phase 1 of the remote proxy deploys with no listen-side auth (anyone
can use it as egress to api.anthropic.com, but they bring their own
key). If a future change enables inject_api_key while listen_key is
still None, the operator's upstream credits become free for anyone
on the internet. Comment is the cheapest mitigation until phase 2
adds per-key auth.
EOF
)"
```

---

## Task 2: Add proxy.aishipbox.com nginx site config

**Files:**
- Create: `deploy/proxy.aishipbox.com.nginx.conf`

- [ ] **Step 1: Create the nginx site file**

Write `deploy/proxy.aishipbox.com.nginx.conf` with this exact content:

```nginx
# Site config for proxy.aishipbox.com — the claude-courier `proxy`
# subcommand fronting api.anthropic.com on 127.0.0.1:3008.
#
# Differences from broker.aishipbox.com.nginx.conf:
#   - proxy_buffering off       -> required so SSE streams flush incrementally
#   - proxy_read_timeout 300s   -> Anthropic streams can run minutes long
#   - proxy_send_timeout 300s   -> symmetric
#   - client_max_body_size 10m  -> prompt payloads with attachments
#
# One-time post-deploy steps (not in CI):
#   1. scp deploy/proxy.aishipbox.com.nginx.conf <user>@<host>:/tmp/
#   2. ssh <user>@<host>
#   3. sudo mv /tmp/proxy.aishipbox.com.nginx.conf \
#        /etc/nginx/sites-available/proxy.aishipbox.com
#   4. sudo ln -s /etc/nginx/sites-available/proxy.aishipbox.com \
#        /etc/nginx/sites-enabled/
#   5. sudo nginx -t && sudo systemctl reload nginx
#   6. Add DNS A record proxy.aishipbox.com -> server IP (Route 53)
#   7. sudo certbot --nginx -d proxy.aishipbox.com
# After step 7 certbot rewrites this file in-place to add the listen 443 ssl
# block; that's expected.

server {
    listen 80;
    listen [::]:80;
    server_name proxy.aishipbox.com;

    client_max_body_size 10m;

    location / {
        proxy_pass http://127.0.0.1:3008;
        proxy_http_version 1.1;

        proxy_set_header Host              $host;
        proxy_set_header X-Real-IP         $remote_addr;
        proxy_set_header X-Forwarded-For   $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        proxy_buffering       off;
        proxy_connect_timeout 5s;
        proxy_send_timeout    300s;
        proxy_read_timeout    300s;
    }
}
```

- [ ] **Step 2: Verify with nginx -t syntactically (optional but cheap)**

If nginx is installed locally (`which nginx`), run:

```sh
nginx -t -c /dev/null -p . -g 'events {}; http { include deploy/proxy.aishipbox.com.nginx.conf; }'
```

Expected: "syntax is ok" / "test is successful". Skip this step if nginx is not installed locally — the real test happens on the server in step 6 of the post-deploy procedure.

- [ ] **Step 3: Commit**

```sh
git add deploy/proxy.aishipbox.com.nginx.conf
git commit -m "$(cat <<'EOF'
docs(deploy): nginx site config for proxy.aishipbox.com

Sibling to broker.aishipbox.com.nginx.conf. Targets 127.0.0.1:3008
(the claude-courier proxy container) with SSE-friendly settings:
proxy_buffering off, 300s read/send timeouts, 10m max body. Manual
post-deploy steps (symlink, reload, certbot) are documented in the
file header.
EOF
)"
```

---

## Task 3: Roll proxy container in deploy.yml

**Files:**
- Modify: `.github/workflows/deploy.yml:28-32` (concurrency group rename)
- Modify: `.github/workflows/deploy.yml:100-156` (remote script: add proxy roll)

- [ ] **Step 1: Rename the concurrency group**

Use Edit to replace:

```yaml
concurrency:
  group: deploy-broker
  cancel-in-progress: false
```

with:

```yaml
concurrency:
  group: deploy
  cancel-in-progress: false
```

Reason: the workflow now manages two containers; the group name should reflect that. `cancel-in-progress: false` stays — we never want to cancel a half-done rollout.

- [ ] **Step 2: Extend the remote script to roll the proxy**

Locate the `Roll container on server` step in `.github/workflows/deploy.yml`. The current remote script ends with:

```yaml
          docker ps --filter name=claude-courier-broker --format 'table {{.Names}}\t{{.Image}}\t{{.Status}}'
          REMOTE
```

Use Edit to replace exactly this text:

```yaml
          docker ps --filter name=claude-courier-broker --format 'table {{.Names}}\t{{.Image}}\t{{.Status}}'
          REMOTE
```

with:

```yaml
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

          echo "waiting for proxy to respond"
          for _ in 1 2 3 4 5 6 7 8 9 10; do
            code=$(curl -s -o /dev/null -w '%{http_code}' -m 2 http://127.0.0.1:3008/ || echo 000)
            # Any 4xx/2xx proves the proxy is serving (Anthropic returns 4xx for `/`).
            if [ "${code#2}" != "$code" ] || [ "${code#4}" != "$code" ]; then
              echo "proxy responding (HTTP $code)"
              break
            fi
            sleep 1
          done

          docker ps --filter name=claude-courier --format 'table {{.Names}}\t{{.Image}}\t{{.Status}}'
          REMOTE
```

Note: the final `docker ps` filter goes from `name=claude-courier-broker` to `name=claude-courier` so it shows both containers in the deploy log.

- [ ] **Step 3: Verify the YAML still parses**

Run from the repo root:

```sh
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/deploy.yml'))" && echo OK
```

Expected: `OK`. If it errors, the most likely cause is heredoc indentation — the `REMOTE` body must remain at column 0 (no leading whitespace before `echo`, `docker`, `for`, `if`, etc. inside the heredoc). Fix and re-run.

- [ ] **Step 4: Sanity-check the rendered remote script**

Eyeball the diff to confirm:
- Both `docker rm -f` lines and both `docker run` blocks live inside the same heredoc (between the opening `cat <<'REMOTE'` and the closing `REMOTE`).
- The proxy `docker run` does NOT pass `--inject-api-key` or `--listen-key`. Phase 1 is open; clients bring their own Anthropic key.
- The proxy port mapping is `127.0.0.1:3008:3008` (not `0.0.0.0:3008` — public exposure happens via nginx, not Docker).

- [ ] **Step 5: Commit**

```sh
git add .github/workflows/deploy.yml
git commit -m "$(cat <<'EOF'
ci: roll claude-courier-proxy container alongside broker

deploy.yml now starts a second container running the `proxy`
subcommand against the anthropic upstream on 127.0.0.1:3008. No
listen-side auth in this phase — the proxy is open and clients send
their own Anthropic x-api-key. nginx (proxy.aishipbox.com) is
configured manually post-deploy; see deploy/proxy.aishipbox.com.nginx.conf.

Concurrency group renamed deploy-broker -> deploy since the workflow
manages two containers now.
EOF
)"
```

---

## Task 4: Push the release tag

**Files:** none (git only)

- [ ] **Step 1: Confirm working tree is clean**

Run: `git status`
Expected: `nothing to commit, working tree clean`. If not, you missed a commit in tasks 1-3 — go back and finish.

- [ ] **Step 2: Pick the next version**

Run: `git tag --list 'v*' --sort=-v:refname | head -3`
Pick the next semver. Phase 1 is a feature add (new container in deploy), so bump minor: e.g. if latest is `v0.1.5`, next is `v0.2.0`.

- [ ] **Step 3: Confirm with the user before pushing**

Pushing the tag triggers both `release.yml` (artifact build) AND `deploy.yml` (server rollout). Stop and ask the user:

> "Ready to tag `v0.X.Y` and push? This will roll the broker container AND start the new proxy container on the server."

Wait for explicit yes. Do NOT push tags autonomously.

- [ ] **Step 4: Tag and push**

```sh
git tag v0.X.Y
git push origin main v0.X.Y
```

- [ ] **Step 5: Watch the deploy**

```sh
gh run watch
```

Expected: both `deploy broker` and `release` workflows succeed. The `Roll container on server` step output should show:
- `pulling ghcr.io/xuopoj/claude-courier:v0.X.Y`
- `broker responding (HTTP 404)` (or 405 — anything 2xx/4xx)
- `proxy responding (HTTP 4xx)` (Anthropic returns 4xx for bare `GET /`)
- A `docker ps` table listing both `claude-courier-broker` and `claude-courier-proxy` as `Up`.

If the proxy container fails to start, SSH to the host and run `docker logs claude-courier-proxy` to see the panic/error. Common causes: image pull failure (check GHCR auth), port 3008 already bound (rare — nothing else uses it), proxy subcommand misspelled in the docker run.

---

## Task 5: One-time nginx + DNS + cert setup on server

**Files:** none on the local machine

This task is executed manually on the server, exactly once. It is not part of CI and never will be in phase 1.

- [ ] **Step 1: Add DNS A record**

In Route 53 (or whichever DNS host), add `proxy.aishipbox.com` → server IP. Verify:

```sh
dig +short proxy.aishipbox.com
```

Expected: the server's public IP. If empty, DNS hasn't propagated — wait 1-5 minutes, retry.

- [ ] **Step 2: Copy the nginx config to the server**

```sh
scp deploy/proxy.aishipbox.com.nginx.conf <user>@<host>:/tmp/
```

Replace `<user>@<host>` with the values from your SSH config (same account that's used by the deploy workflow).

- [ ] **Step 3: Install the nginx site**

SSH to the host:

```sh
ssh <user>@<host>
```

Then on the server:

```sh
sudo mv /tmp/proxy.aishipbox.com.nginx.conf /etc/nginx/sites-available/proxy.aishipbox.com
sudo ln -s /etc/nginx/sites-available/proxy.aishipbox.com /etc/nginx/sites-enabled/
sudo nginx -t
```

Expected: `syntax is ok` and `test is successful`. If not, fix the file on the server (or scp a corrected version) and re-test.

- [ ] **Step 4: Reload nginx**

```sh
sudo systemctl reload nginx
```

Expected: no output, exit 0. Verify with `sudo systemctl status nginx` — should be `active (running)` with no recent errors.

- [ ] **Step 5: Acquire TLS cert via certbot**

```sh
sudo certbot --nginx -d proxy.aishipbox.com
```

Follow the interactive prompts: pick the email if asked, agree to ToS, choose redirect HTTP→HTTPS when offered (recommended). certbot rewrites `/etc/nginx/sites-available/proxy.aishipbox.com` to add a `listen 443 ssl` block. This is expected — the in-repo file is the **pre-cert** template; the server's copy diverges after this step.

- [ ] **Step 6: Verify TLS**

From your laptop:

```sh
curl -sI https://proxy.aishipbox.com/ | head -1
```

Expected: any HTTP status line — `HTTP/2 404`, `HTTP/2 405`, etc. The point is that TLS handshake succeeded and nginx is forwarding.

- [ ] **Step 7: End-to-end smoke test (Anthropic streaming)**

```sh
export ANTHROPIC_KEY=sk-ant-...   # your real Anthropic key
curl -N https://proxy.aishipbox.com/v1/messages \
  -H "x-api-key: $ANTHROPIC_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -H "content-type: application/json" \
  -d '{"model":"claude-opus-4-7","max_tokens":64,"stream":true,"messages":[{"role":"user","content":"hi"}]}'
```

Expected: SSE frames arrive incrementally (you see lines like `event: message_start`, `event: content_block_delta`, etc., with visible streaming — not one big dump at the end). The response completes within seconds and ends with `event: message_stop`.

If you see all the events at once at the end (instead of streaming), `proxy_buffering off` didn't take — re-check the nginx file on the server. If you see 502, `docker logs claude-courier-proxy` on the host. If you see 504, the upstream is slow or `proxy_read_timeout` is too low.

- [ ] **Step 8: End-to-end non-streaming test**

```sh
curl -s https://proxy.aishipbox.com/v1/messages \
  -H "x-api-key: $ANTHROPIC_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -H "content-type: application/json" \
  -d '{"model":"claude-opus-4-7","max_tokens":64,"messages":[{"role":"user","content":"hi"}]}'
```

Expected: a single JSON response with a `content` array. If this works AND step 7 streamed correctly, phase 1 is done.

---

## Definition of done

- [ ] `cargo build` and `cargo test` pass on `main`.
- [ ] Tag `v0.X.Y` pushed; both GitHub Actions workflows succeed.
- [ ] `docker ps` on the server lists `claude-courier-broker` AND `claude-courier-proxy`, both `Up`.
- [ ] `https://proxy.aishipbox.com/v1/messages` answers a streaming Anthropic request with visible SSE.
- [ ] Cert for `proxy.aishipbox.com` is valid (`sudo certbot certificates` shows it, expiry > 30 days).

When all five boxes are checked, phase 1 is shipped. Phase 2 (per-key issuance + usage logging) is a separate spec/plan.
