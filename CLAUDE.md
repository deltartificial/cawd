# cawd

cawd (Code Aware Workspace Display) is a Rust terminal file explorer with syntax
highlighting, built on `ratatui` + `crossterm`. It is a single binary crate
(`cawd/`) inside a strict Rust workspace.

## Development Workflow

### Build

```bash
cargo build                  # default members only
cargo build --workspace      # entire workspace
make build                   # via Makefile
make maxperf                 # max performance build (fat LTO, codegen-units=1)
```

### Run

```bash
cargo run -- [path]          # run cawd on a directory
cargo install --path cawd    # install the `cawd` binary
```

### Test

```bash
cargo nextest run --workspace          # unit tests
cargo test --doc --workspace           # doctests (nextest doesn't run these)
make test                              # all tests via Makefile
```

### Lint

```bash
cargo +nightly fmt --all               # format
cargo +nightly clippy --workspace --all-targets --all-features  # lint
make lint                              # all linters (fmt, clippy, typos, dprint)
make pr                                # full pre-PR check
```

### Pre-PR Checklist

1. `make lint` passes
2. `make test` passes
3. `cargo doc --workspace --all-features --no-deps --document-private-items` builds without warnings
4. PR title follows conventional commits: `type(scope): description`

## Code Style

- Follow Rust 2024 edition idioms
- Use `tracing` for logging, never `println!` or `eprintln!` (the UI renders
  through `ratatui`, so production code should not print directly)
- Prefer `#[must_use]` on functions returning values that should not be ignored
- Comments should explain **why**, not **what**
- Keep public APIs well-documented with `///` doc comments

## Commit Convention

Conventional Commits format: `type(scope): description`

Allowed types: `feat`, `fix`, `chore`, `test`, `bench`, `perf`, `refactor`, `docs`, `ci`, `revert`, `deps`

Breaking changes: append `!` after type, e.g. `feat!: remove deprecated API`

## Git Workflow

- All changes go through pull requests — never push directly to main
- PRs are squash-merged so the commit shows: `type(scope): description (#PR)`
- Keep commit history clean and linear

## Architecture

Rust workspace with strict tooling:

- **clippy** (nightly, nursery lints enabled)
- **rustfmt** (nightly, `imports_granularity = "Crate"`)
- **cargo-deny** (license auditing, ban openssl)
- **dprint** (TOML/markdown/JSON formatting)
- **typos** (spell checking with hex address exclusions)
- **nextest** (test runner with retry/timeout policies)
- **cross** (cross-compilation for aarch64, riscv64)

## CI

CI runs on every PR and push to main:

- **lint.yml**: clippy, fmt, docs, typos, deny, dprint, with `lint-success` gate
- **test.yml**: nextest, doctest, MSRV (1.93) check, with `test-success` gate
- **pr-title.yml**: conventional commit enforcement on PR titles
