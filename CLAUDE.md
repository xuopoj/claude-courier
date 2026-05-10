# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build / test / run

```sh
cargo build                          # debug
cargo build --release                # what install.sh ships; LTO + strip + opt-level=z
cargo test                           # all tests (currently 3 config round-trip tests)
cargo test --test config_test        # single test file
cargo test broker_config_roundtrip   # single test by name
./target/debug/claude-courier --help # see all subcommands

# Local end-to-end smoke (broker + publish from real Keychain + sandbox consume):
./target/debug/claude-courier broker --bind 127.0.0.1:3019 --publish-key P --consume-key C &
SANDBOX=$(mktemp -d) && HOME=$SANDBOX ./target/debug/claude-courier consume \
  --broker http://127.0.0.1:3019 --key C
```

## Architecture

Five roles share a single binary; the `Cmd` enum in `src/main.rs` dispatches to one of:

- **publisher** (`src/client.rs::publish`) — shells out to `security find-generic-password -s 'Claude Code-credentials' -w` to read a JSON blob from macOS Keychain, reads identity fields from `~/.claude.json`, validates `claudeAiOauth.accessToken`, and POSTs `{credentials, identity}` to `<broker>/publish` with `x-api-key`. macOS-only at runtime (the Keychain read errors elsewhere).
- **consumer** (`src/client.rs::consume`) — GETs `<broker>/consume`, validates the envelope, atomically writes `~/.claude/.credentials.json` (mode 0600 via tmpfile + rename), shallow-merges identity into `~/.claude.json` preserving pre-existing fields. Cross-platform.
- **broker** (`src/broker.rs`) — hyper HTTP/1.1 server. Storage is a single `Mutex<Option<Bytes>>` slot, in-memory only. `POST /publish` overwrites; `GET /consume` returns the slot **without clearing it** — that's intentional so multiple consumer machines can each fetch the latest token. `410 Gone` only means "nothing has ever been published," not "consumed already." Separate `publish_key` and `consume_key` so a leaked consumer can't write.
- **router** (`src/router.rs`) — hyper inbound + reqwest outbound. Validates incoming `x-api-key` (or `x-courier-key`) against a list of named per-consumer keys (constant-time compare via `subtle::ConstantTimeEq`), strips it, fetches a fresh OAuth token from the broker on demand (cached in `Mutex<Option<CachedToken>>`, refetched when `expiresAt < now + expiry_buffer_secs`), and forwards to `https://api.anthropic.com` with `Authorization: Bearer <token>` + `anthropic-beta: oauth-2025-04-20`. Streams responses (SSE works). Refuses to start with zero enabled keys. Logs include `key=<name>` per request — name comes from `RouterKey { name, key, disabled }` entries in `router.toml`. Token refresh is request-driven, not background-timed.
- **proxy** (`src/proxy.rs`) — unrelated to the token-sync flow. hyper inbound + reqwest outbound; streams responses back via `bytes_stream() → StreamBody` (SSE works). Built-in upstream presets in `resolve_upstream()`: `anthropic|claude`, `openai`, `gemini|google`. Strips hop-by-hop headers and the listen-side `x-api-key` (so a local auth secret never leaks upstream). Optional `inject_api_key` only fires when the client sent neither `x-api-key` nor `authorization` — Anthropic upstreams get `x-api-key`, others get `Authorization: Bearer`.

Each role is independently configurable via TOML in `~/.config/claude-courier/{publisher,broker,consumer,router,proxy}.toml` (or `~/Library/Application Support/...` on macOS — `dirs::config_dir()` chooses). `<role>-configure` subcommands write the TOML; `<role>` subcommands read it. CLI flags always override file values via the `resolve_*` functions in `src/config.rs`. The broker also accepts `--publish-key` / `--consume-key` directly (no toml needed) so the Docker container can run with flags only. The router's `keys` list is managed via the dedicated `router-key {add,list,revoke,rm}` subcommands (in `router_key_action()` in `src/main.rs`); `add` generates 32 bytes via `getrandom` and prints the key once.

`src/http.rs` is shared client glue (`auth_get`, `auth_post_json`, `FetchResult::{Ok,Gone}`). `src/log.rs` is a one-line timestamped `eprintln`. The lib (`src/lib.rs`) re-exports everything so integration tests in `tests/` can poke internals.

## Deploy & release pipeline

Three GitHub Actions workflows:

- `.github/workflows/deploy.yml` — **tag-triggered** (`v*` push). Builds Docker image, pushes to `ghcr.io/xuopoj/claude-courier:<tag>` + `:latest`, SSHes to the server defined in repo secrets (`SSH_HOST`, `SSH_USER`, `SSH_PRIVATE_KEY`, `SSH_PORT?`, `BROKER_PUBLISH_KEY`, `BROKER_CONSUME_KEY`), and rolls `claude-courier-broker` container. Uses `StrictHostKeyChecking=accept-new` (trust-on-first-use every deploy because runners are ephemeral — see the workflow header for the trade-off note). Remote script values are passed via `printf %q` quoting to survive shell-special chars in keys.
- `.github/workflows/deploy-router.yml` — **manual trigger only** (`workflow_dispatch` with optional `tag` input, defaulting to `latest`). Pulls the image, rolls `claude-courier-router` container on `127.0.0.1:3008`, mounts `/etc/claude-courier/router.toml` read-only into the container at the same path with `XDG_CONFIG_HOME=/etc` so `dirs::config_dir()` resolves correctly. The host file must be chowned to `10001:10001` (the `courier` uid) and 0600. `router.toml` itself (with `[[keys]]`) is **never** in CI; you `scp` it manually after running `router-key add` locally. To rotate keys: edit/regenerate locally → scp → `docker restart claude-courier-router`. The workflow refuses to start the container if `/etc/claude-courier/router.toml` is missing.
- `.github/workflows/release.yml` — **tag-triggered**. Matrix-builds three targets and attaches tarballs + `.sha256` sidecars to the GitHub Release: `aarch64-apple-darwin` (native macos-latest), `x86_64-unknown-linux-gnu` (native ubuntu-latest), `aarch64-unknown-linux-gnu` (cross-built via `cross` from the x86_64 runner).

Routine commits to `main` do **not** deploy. Cut a release with `git tag v0.x.y && git push --tags`. The two tag-triggered workflows fire in parallel; `deploy-router.yml` is independent and you trigger it manually after pushing config changes.

The Dockerfile is multi-stage rust:bookworm → debian-slim, runs as a non-root `courier` user (uid 10001, `--no-create-home`), exposes 3007. Default CMD is `broker`. The broker binds to `0.0.0.0:3007` inside the container and is published to `127.0.0.1:3007` on the host. The router (when deployed) binds to `0.0.0.0:3008` inside the container and is published to `127.0.0.1:3008` on the host. TLS terminates in front via nginx (configured directly on the server, not vendored in this repo) — `broker.aishipbox.com` and `router.aishipbox.com` are separate vhosts.

`install.sh` is the consumer-side installer; it calls the GitHub `/releases/latest` redirect (or honors `VERSION=v...`), picks the matching tarball by detected `uname -s/-m`, verifies sha256, and installs to `/usr/local/bin` (sudo if needed) or `~/.local/bin` (`NO_SUDO=1` or fallback).

## Things to know when editing

- `Cargo.toml` uses `edition = "2024"` and pins `tokio = { features = ["full"] }`. Don't downgrade for "minimalism" — async-process spawn (`tokio::process`) and signal handling are needed.
- Release profile is `opt-level = "z" + lto + strip` to keep the released binary ~1.5 MB. Don't change without thinking — install.sh users download these.
- When touching `src/proxy.rs` or `src/router.rs`: streaming is via `reqwest::Response::bytes_stream() → futures::TryStreamExt::map_ok(Frame::data) → StreamBody::boxed()`. Don't accidentally collect-then-emit (breaks SSE).
- The broker's slot and the router's token cache are both `Arc<State>` with a `tokio::sync::Mutex`. Locks are held only briefly (clone for read, assignment for write); don't introduce long-held locks.
- The router's key list is read once at startup into `RouterState.keys` and never reloaded. To rotate keys, restart the process (or container). Hot-reload would need a SIGHUP path; not built.
- Identity merge in `client.rs::merge_identity` is **shallow** — fields in `identity` overwrite top-level fields in `~/.claude.json` but other top-level fields are preserved. Intentional. Don't change to deep-merge without thinking through what happens to `oauthAccount` (which is itself an object).
- The `*-configure` subcommands write TOML with mode `0600` via `write_secure()` in `src/config.rs`. The `router-key` subcommands also use it. Anything new that touches secrets-on-disk should reuse it.
- Tests live in `tests/config_test.rs` and only cover TOML round-trips (5 tests, including `router_config_roundtrip` and `router_config_accepts_missing_keys_field` which proves `#[serde(default)]` on the `keys` Vec works for fresh configs). Network/IO behavior (broker, router, proxy, Keychain) is not unit-tested — verify behavior changes by running the local smoke at the top of this file and/or the `cargo test` suite.

## Dotfiles to leave alone

- `LightsailDefaultKey-us-west-2.pem` is gitignored (`*.pem`) but lives in the working tree. It's an AWS SSH private key. Don't `git add` it. Don't echo it. If it's ever staged, unstage immediately.
- `~/Library/Application Support/claude-courier/*.toml` on the user's Mac contains real broker URLs and keys. Don't print full file contents in tool output; show shape only.
- `router.toml` specifically contains a `[[keys]]` array of plaintext per-consumer secrets. Same rule — show shape, never `cat` it. If you regenerate or rotate, the new key is printed once by `router-key add`; don't echo or commit that either.
