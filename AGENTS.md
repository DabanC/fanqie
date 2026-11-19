# Working On Fanqie

## Project Scope

Fanqie is a Rust terminal novel reader inspired by Claude Code's interaction style. Keep online novel search and full-chapter reading as first-class features; do not replace them with a local-only reader or require a user-supplied API for normal operation.

- Rust 2021, minimum compiler declared in `Cargo.toml` (currently 1.89).
- Ratatui and Crossterm provide the terminal UI.
- Reqwest I/O runs on cancellable background workers, never in drawing or key handlers.
- User-facing text is Chinese. Preserve the existing warm-toned design and keyboard conventions unless the task explicitly changes them.
- The application runs entirely in Rust. Do not introduce Python or Node.js runtime sidecars.

## Code Map

| Location | Responsibility |
| --- | --- |
| `src/main.rs` | Startup wiring and session-only CLI overrides |
| `src/cli.rs` | Clap arguments and argument-validation tests |
| `src/terminal.rs` | Terminal setup/cleanup, event loop, periodic progress saves |
| `src/app/` | Application state, worker results, commands, input, pickers, reading progress |
| `src/ui/` | Rendering, palette, home/reader views, input and picker presentation |
| `src/library/` | Library facade, local imports, search orchestration and source adapters |
| `src/library/app_api/` | Anonymous official APP device, signing, search and chapter protocol |
| `src/library/web/` | Website/custom-API access, catalog/content parsing and web-font decoding |
| `src/net.rs` | Cancellable, bounded HTTP requests and pacing waits |
| `src/models.rs` | Validated book/chapter/settings/progress/search-report types |
| `src/storage.rs` | Atomic persistence, hashed cache paths and search-name cache |
| `licenses/` | Required notices for adapted third-party implementations |

Keep source protocols behind `Library`; the UI and application controller should not build URLs, sign requests or parse upstream responses. Use normal Rust module declarations instead of `#[path]` aliases. Prefer small, cohesive modules and explicit imports. Give helpers the narrowest useful visibility; avoid new public APIs or dependencies solely to simplify a refactor.

## Build And Validation

Run from the repository root after a coherent change:

```sh
cargo fmt --all -- --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo build
```

Apply formatting with `cargo fmt --all` when needed. Keep `Cargo.lock` with this executable project. Preserve deterministic tests when moving code, including exact test paths used by child-process tests.

Offline interactive smoke test:

```sh
cargo run -- --demo --data-dir "$(mktemp -d)"
```

Verify scrolling, chapter navigation, `/toc`, `/shelf`, `/theme`, command editing, and progress restoration. Rendering changes should also pass `TestBackend` checks at narrow and normal terminal sizes. Do not require a live service for normal tests.

### Live Tests Are Opt-In

Network tests are marked `#[ignore]`. Never run all ignored tests indiscriminately: some register an anonymous device or require explicit environment variables. Run only the relevant test and prefer an isolated data directory:

```sh
DATA_DIR="$(mktemp -d)"
FANQIE_LIVE_DATA_DIR="$DATA_DIR" cargo test live_reported_title_search -- --ignored --nocapture
```

Read each live test before invoking it. Search tests verify public metadata; chapter tests must check substantial, complete text, the requested book/chapter identity, no residual private-use glyphs, and cache equality. A passing fallback test does not establish that the primary APP endpoint recovered. Report the actual source, degraded mode and any unverified behavior.

## Behavior To Preserve

### Search And Source Reliability

- Identify books by book ID, not title. One book can have an APP display name, an original name and a different website title.
- Keep exact-title ranking, alias handling and ID deduplication. Never treat a similarly named book as the requested one.
- Preserve bounded pagination, same-device transient retries, and explicit degraded-result notices. Keep already-fetched valid pages when a later page fails.
- An HTTP 200 with an empty body is a failed request, not an empty search result or proof that a device expired.
- Honor explicit rate limits and persisted cooldowns. Do not route around them, renew devices on empty responses, or create unbounded retries.
- Respect an explicitly configured custom API; do not silently replace it with a built-in service.
- Do not claim that non-public official endpoints are stable or officially supported developer APIs.

### Reading And Terminal Safety

- Fetch chapters on demand. The optional next-chapter lookahead must fetch at most one adjacent chapter near the end of the current chapter; never crawl a book in the background.
- A website's locked/short preview is not a full chapter. Preserve the complete-content checks and appropriate source fallback.
- Only decode known, verified web fonts. Keep the mapping, protocol constants, cryptographic vectors and third-party notices intact when reorganizing files.
- Never execute upstream scripts. Validate identities, strip unsafe control characters and refuse unresolved font-obfuscated text.
- Preserve generation checks that discard stale worker results. Network work must not freeze the terminal.
- Restore raw mode, alternate screen, bracketed paste and cursor state on exit or panic. Attempt the final progress save even after terminal I/O errors.
- Keep logical reading position through window resizes; do not discard live reading state if saving progress fails.

### Persistence And Credentials

- Preserve existing state versions, filenames and serialized formats unless a migration is explicitly part of the task.
- Use atomic writes and hashed book/chapter/search-cache filenames. Catalog updates must not overwrite reading progress.
- Search metadata belongs in the search index, not the user's shelf until the book is opened/imported.
- Corrupt settings may contain API credentials: fail closed and never overwrite them with defaults. Report recoverable cache damage without deleting the original files.
- Anonymous device/key files require private permissions (Unix mode 0600 or owner-only Windows ACLs) and process locking. Do not delete credentials or reading data to make a test pass.
- CLI API overrides stay session-only. Only explicit settings commands should persist those API values; changing the theme/width must not leak session credentials to disk.
- Never log credentials, decrypted keys, signed URLs, token-bearing API URLs or private response bodies. Diagnostics should use safe error categories, HTTP status and response size.

## Change Discipline

Make structural cleanup behavior-preserving and review moves against the original source. Keep unrelated changes out of the diff. Put regression tests near the module that owns the behavior; use sibling `tests.rs` files when inline tests obscure the implementation. Explain non-obvious protocol or persistence constraints briefly rather than narrating straightforward code. Do not add unrelated frameworks, generated documentation or configuration files.
