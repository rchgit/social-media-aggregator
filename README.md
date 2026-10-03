# social-media-aggregator (`sma`)
A Rust TUI that aggregates posts from multiple social networks into single
topic-filtered feed. Multiple accounts per network, normalized post model,
persistent SQLite storage, and a `ratatui` interface.

## Networks

| Network | Adapter | Notes |
|---------|---------|-------|
| Facebook | Meta Graph API v20.0 | page/user id as handle |
| Reddit | public JSON API | works without auth; optional OAuth bearer |
| X / Twitter | API v2 | bearer token; handle resolved to user id |
| Instagram | Meta Graph API v20.0 | Business/creator account id as handle |
| Threads | Threads API v1.0 | user id as handle |
| TikTok | Display API v2 | bearer token; cover image surfaced |

Only text and image posts are modeled in this first release; video is skipped.

## Build

A `mise.toml` pins the Rust toolchain (currently `1.98.0`) so anyone with
[mise](https://mise.jdx.dev/) gets a reproducible build:

```bash
mise install         # one-time, installs pinned Rust
mise exec -- cargo build --release
```

On a plain machine without mise:

```bash
cargo build --release
```

binary is `target/release/sma`.

> **Linker note:** this project was built on host that ships runtime-only
> glibc (no `glibc-devel`). Local glibc-devel shim is unpacked under
> `~/.local/share/sma-sysroot` and referenced from `.cargo/config.toml`. On a
> normal development machine you can delete `.cargo/config.toml` and the
> `sma-sysroot` directory; project builds with system toolchain.

## Quick start

```bash
# write a sample config (defaults to ./sma.toml, overridable with --config / $SMA_CONFIG)
sma config init

# add accounts (credentials are never stored on disk; see "Secrets" below)
sma accounts add --network reddit --handle spez
sma accounts add --network x --handle elonmusk --secret X_BEARER

# add topics (comma-separated keywords)
sma topics add --name rust --keywords rust,programming,cargo
sma topics add --name ai --keywords ai,llm,gpt

# fetch posts
sma sync

# read the feed for a topic
sma feed --name rust

# or launch the TUI (no subcommand)
sma
```

## TUI

| Key | Action |
|-----|--------|
| `1` `2` `3` `4` | accounts / topics / feed / sync screens |
| `j` / `k` or arrows | move selection |
| `a` | add (account or topic, depending on screen) |
| `d` | delete selected |
| `e` | toggle enabled |
| `t` | cycle topic (feed screen) |
| `n` / `p` | next / previous page (feed screen) |
| `enter` | open post detail (feed screen) |
| `s` | run sync (sync screen) |
| `q` / `esc` | back / quit |

Post detail renders attached images as truecolor half-block preprints and
emits OSC 8 clickable links for the post URL.

## CLI

```
sma config init                      # write sample config
sma config path                      # print resolved config path
sma accounts add|list|remove|enable|disable
sma topics   add|list|remove|enable|disable
sma sync [--account HANDLE]
sma feed --name TOPIC [--limit N] [--offset N]
```

## Configuration

```toml
[settings]
db_path = "sma.db"
cache_dir = "sma-cache"
page_size = 50

[secrets]
# Named commands that emit a credential on stdout.
# Reference them from an account's `--secret` as `cmd:<name>`.
#   [secrets.commands]
#   twitter = "pass show social/twitter"
```

## Secrets

A credential reference is one of:

- `NAME` or `env:NAME` — read the environment variable `NAME`.
- `cmd:provider` — run the command registered under `provider` in
  `config.secrets.commands` and use its trimmed stdout.

Secrets are resolved at sync time and never written to disk.

## Storage

SQLite with embedded migrations. Posts are deduplicated by
`(network, external_id)`. The feed for a topic is ordered newest-first with
deterministic tie-breaking on `(created_at, network, external_id)`. Per-account
sync cursors are persisted so each run picks up where the last left off.

## Verification

```bash
cargo fmt --check
cargo test --all-targets
cargo clippy --all-targets --all-features
```
