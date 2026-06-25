# Agents Guide

This file provides context for AI agents (Copilot, Claude, etc.) working on this codebase.

## Project overview

Galvaan is a Rust CLI tool that keeps desktop apps up to date by tracking GitHub releases and installing them via system package managers. It supports multiple Linux distributions.

## Architecture

```
src/
├── main.rs              # Entry point, command handlers, CLI orchestration
├── cli/mod.rs           # Clap CLI definitions (commands, flags, completions)
├── config/mod.rs        # TOML config: Settings, TrackedApp, enums
├── github/mod.rs        # GitHub API client, release fetching, asset download
├── logging.rs           # Tracing-based file logging
└── package_manager/
    ├── mod.rs           # PackageManager trait + factory
    ├── zypper.rs        # openSUSE (zypper)
    ├── dnf.rs           # Fedora/RHEL (dnf)
    ├── apt.rs           # Debian/Ubuntu (apt-get)
    └── pacman.rs        # Arch Linux (pacman)
```

## Key design decisions

- **Trait-based package managers**: All PMs implement `PackageManager` trait with `install()`, `installed_version()`, `name()`. Add new PMs by creating a new file and adding to the `create()` factory.
- **Config testability**: `Config::load_from(path)` allows tests to use temp directories instead of the real config path.
- **Auto-approve modes**: `Always` (always -y), `NoDeps` (auto-approve only when no new deps), `Never` (always prompt). Implemented via dry-run + output parsing per PM.
- **Integration tests use containers**: Real distro environments via podman/docker. Tests are `#[ignore]` — run with `cargo test --test container_integration -- --ignored`.

## Building and testing

```bash
# Build
cargo build --release

# Unit tests (fast, no network/containers needed)
cargo test

# Integration tests (requires podman + network)
cargo build --release
cargo test --test container_integration -- --ignored
```

## Adding a new package manager

1. Create `src/package_manager/<name>.rs` implementing the `PackageManager` trait
2. Add variant to `PackageManagerType` enum in `src/config/mod.rs`
3. Update `FromStr` impl for the new variant
4. Update `create()` factory in `src/package_manager/mod.rs`
5. Add a Containerfile in `tests/integration/distros/`
6. Add distro const + test function in `tests/container_integration.rs`
7. Update README

## Adding a new command

1. Add variant to `Commands` enum in `src/cli/mod.rs`
2. Add handler function `cmd_<name>()` in `src/main.rs`
3. Wire it up in the `match` in `main()`
4. Add CLI parsing tests

## Code style

- Rust 2024 edition
- No unnecessary comments — code should be self-explanatory
- Tests live in `#[cfg(test)] mod tests` at the bottom of each module
- Integration tests are separate files in `tests/`

## Config location

`~/.config/galvaan/config.toml` — managed via `galvaan config show/set/path`

## Common pitfalls

- `HashMap::get_mut(&app_name)` where `app_name` is `&String` from iterator causes double-reference — use `get_mut(app_name)` directly
- APT uses `apt-get` (not `apt`) for scripting stability
- Pacman has no convenient dry-run for dep checking — `NoDeps` behaves like `Always`
- The `tracing-appender` guard must be kept alive in main or logging stops
- Container integration tests run in parallel — use unique filenames in shared context directories
