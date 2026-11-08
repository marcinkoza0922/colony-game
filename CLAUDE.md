## Build and test

- Tests: `cargo nextest run`. Never `cargo test`, except `cargo test --doc` for doctests (nextest doesn't run them).
- Lint: `cargo clippy --all-targets --message-format=short -q -- -D warnings`
- Build check: `cargo check --message-format=short -q`
- Output is kept minimal by `.config/nextest.toml` and the `env` block in `.claude/settings.json`.
- On failure only, re-run just the failing test or crate with full output (e.g. `cargo nextest run -p <crate> <test>`). Don't dump full build logs.
- Backtraces are disabled; for a panic, set `RUST_BACKTRACE=1` for that single command only.
- Use `set -o pipefail` when piping cargo output through `tail` or `grep`, so exit codes aren't lost.
- Never run `cargo clean` unless asked.
