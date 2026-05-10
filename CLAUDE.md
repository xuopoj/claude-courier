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

Four roles share a single binary; the `Cmd` enum in `src/main.rs` dispatches to one of:

- **publisher** (`src/client.rs::publish`) — shells out to `security find-generic-password -s 'Claude Code-credentials' -w` to read a JSON blob from macOS Keychain, reads identity fields from `~/.claude.json`, validates `claudeAiOauth.accessToken`, and POSTs `{credentials, identity}` to `<broker>/publish` with `x-api-key`. macOS-only at runtime (the Keychain read errors elsewhere).
- **consumer** (`src/client.rs::consume`) — GETs `<broker>/consume`, validates the envelope, atomically writes `~/.claude/.credentials.json` (mode 0600 via tmpfile + rename), shallow-merges identity into `~/.claude.json` preserving pre-existing fields. Cross-platform.
- **broker** (`src/broker.rs`) — hyper HTTP/1.1 server. Storage is a single `Mutex<Option<Bytes>>` slot, in-memory only. `POST /publish` overwrites; `GET /consume` returns the slot **without clearing it** — that's intentional so multiple consumer machines can each fetch the latest token. `410 Gone` only means "nothing has ever been published," not "consumed already." Separate `publish_key` and `consume_key` so a leaked consumer can't write.
- **proxy** (`src/proxy.rs`) — unrelated to the token-sync flow. hyper inbound + reqwest outbound; streams responses back via `bytes_stream() → StreamBody` (SSE works). Built-in upstream presets in `resolve_upstream()`: `anthropic|claude`, `openai`, `gemini|google`. Strips hop-by-hop headers and the listen-side `x-api-key` (so a local auth secret never leaks upstream). Optional `inject_api_key` only fires when the client sent neither `x-api-key` nor `authorization` — Anthropic upstreams get `x-api-key`, others get `Authorization: Bearer`.

Each role is independently configurable via TOML in `~/.config/claude-courier/{publisher,broker,consumer,proxy}.toml` (or `~/Library/Application Support/...` on macOS — `dirs::config_dir()` chooses). `<role>-configure` subcommands write the TOML; `<role>` subcommands read it. CLI flags always override file values via the `resolve_*` functions in `src/config.rs`. The broker also accepts `--publish-key` / `--consume-key` directly (no toml needed) so the Docker container can run with flags only.

`src/http.rs` is shared client glue (`auth_get`, `auth_post_json`, `FetchResult::{Ok,Gone}`). `src/log.rs` is a one-line timestamped `eprintln`. The lib (`src/lib.rs`) re-exports everything so integration tests in `tests/` can poke internals.

## Deploy & release pipeline

Two GitHub Actions workflows, both **tag-triggered only** (`v*` push):

- `.github/workflows/deploy.yml` — builds Docker image, pushes to `ghcr.io/xuopoj/claude-courier:<tag>` + `:latest`, SSHes to the server defined in repo secrets (`SSH_HOST`, `SSH_USER`, `SSH_PRIVATE_KEY`, `SSH_PORT?`, `BROKER_PUBLISH_KEY`, `BROKER_CONSUME_KEY`), and rolls `claude-courier-broker` container. Uses `StrictHostKeyChecking=accept-new` (trust-on-first-use every deploy because runners are ephemeral — see the workflow header for the trade-off note). Remote script values are passed via `printf %q` quoting to survive shell-special chars in keys.
- `.github/workflows/release.yml` — matrix-builds three targets and attaches tarballs + `.sha256` sidecars to the GitHub Release: `aarch64-apple-darwin` (native macos-latest), `x86_64-unknown-linux-gnu` (native ubuntu-latest), `aarch64-unknown-linux-gnu` (cross-built via `cross` from the x86_64 runner).

Routine commits to `main` do **not** deploy. Cut a release with `git tag v0.x.y && git push --tags`. Both workflows fire in parallel.

The Dockerfile is multi-stage rust:bookworm → debian-slim, runs as a non-root `courier` user, exposes 3007. Default CMD is `broker`. The broker binds to `0.0.0.0:3007` inside the container and is published to `127.0.0.1:3007` on the host — TLS terminates in front via nginx (see `deploy/broker.aishipbox.com.nginx.conf`).

`install.sh` is the consumer-side installer; it calls the GitHub `/releases/latest` redirect (or honors `VERSION=v...`), picks the matching tarball by detected `uname -s/-m`, verifies sha256, and installs to `/usr/local/bin` (sudo if needed) or `~/.local/bin` (`NO_SUDO=1` or fallback).

## Things to know when editing

- `Cargo.toml` uses `edition = "2024"` and pins `tokio = { features = ["full"] }`. Don't downgrade for "minimalism" — async-process spawn (`tokio::process`) and signal handling are needed.
- Release profile is `opt-level = "z" + lto + strip` to keep the released binary ~1.5 MB. Don't change without thinking — install.sh users download these.
- When touching `src/proxy.rs`: streaming is via `reqwest::Response::bytes_stream() → futures::TryStreamExt::map_ok(Frame::data) → StreamBody::boxed()`. Don't accidentally collect-then-emit (breaks SSE).
- The broker's slot is shared via `Arc<State>` with a `tokio::sync::Mutex`. It's held only briefly (one `clone()` for read, one assignment for write); don't introduce long-held locks.
- Identity merge in `client.rs::merge_identity` is **shallow** — fields in `identity` overwrite top-level fields in `~/.claude.json` but other top-level fields are preserved. Intentional. Don't change to deep-merge without thinking through what happens to `oauthAccount` (which is itself an object).
- The `*-configure` subcommands write TOML with mode `0600` via `write_secure()` in `src/config.rs`. Anything new that touches secrets-on-disk should reuse it.
- Tests live in `tests/config_test.rs` and only cover TOML round-trips. Network/IO behavior (broker, proxy, Keychain) is not unit-tested — verify behavior changes by running the local smoke at the top of this file and/or the `cargo test` suite.

## Dotfiles to leave alone

- `LightsailDefaultKey-us-west-2.pem` is gitignored (`*.pem`) but lives in the working tree. It's an AWS SSH private key. Don't `git add` it. Don't echo it. If it's ever staged, unstage immediately.
- `~/Library/Application Support/claude-courier/*.toml` on the user's Mac contains real broker URLs and keys. Don't print full file contents in tool output; show shape only.
