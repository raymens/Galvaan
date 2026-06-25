# Galvaan

Keep apps up to date based on GitHub releases. Supports multiple Linux distributions and package managers.

## Supported distros & package managers

| Distro family | Package manager | Asset format |
|---------------|----------------|--------------|
| openSUSE      | zypper (default) | `.rpm`     |
| Fedora / RHEL | dnf            | `.rpm`       |
| Debian / Ubuntu | apt          | `.deb`       |
| Arch Linux    | pacman         | `.tar.gz` / `.pkg.tar.zst` |

## Quick start: GitHub Copilot app

```bash
# openSUSE / Fedora (RPM)
galvaan add github/app \
  --name github-copilot \
  --asset-pattern "*-linux-x64.rpm"

# Debian / Ubuntu (DEB)
galvaan add github/app \
  --name github-copilot \
  --asset-pattern "*-linux-x64.deb"

# Check if a new version is available
galvaan check github-copilot

# Download and install the latest release
galvaan update github-copilot
```

## Usage

```bash
# Add an app to track
galvaan add <owner/repo> --asset-pattern "<glob>" [--name <friendly-name>]

# List tracked apps
galvaan list

# Check for available updates (all or specific)
galvaan check [app-name]

# Update apps (downloads + installs via your package manager)
galvaan update [app-name]

# Remove a tracked app
galvaan remove <app-name>
```

### Global flags

```bash
# Suppress package manager output
galvaan -q update

# Override auto-approve for this run
galvaan --auto-approve always update
galvaan --auto-approve never update
```

## Examples

```bash
# GitHub Copilot desktop app (RPM, x64 — openSUSE/Fedora)
galvaan add github/app --name github-copilot --asset-pattern "*-linux-x64.rpm"

# GitHub Copilot desktop app (DEB, x64 — Debian/Ubuntu)
galvaan add github/app --name github-copilot --asset-pattern "*-linux-x64.deb"

# GitHub Copilot desktop app (RPM, ARM64)
galvaan add github/app --name github-copilot --asset-pattern "*-linux-arm64.rpm"

# Explicit package manager override
galvaan add github/app --name github-copilot --asset-pattern "*-linux-x64.rpm" --package-manager dnf

# Track prereleases
galvaan add owner/beta-app --asset-pattern "*.rpm" --prerelease

# Pin to a major version
galvaan add owner/stable-app --asset-pattern "*.rpm" --pin "1.*"
```

## Version pinning

Pin an app to a specific version or range to control which releases are offered:

```bash
# Pin to exact version
galvaan pin myapp 1.0.24

# Pin to major version (any 1.x.x)
galvaan pin myapp "1.*"

# Pin to semver range
galvaan pin myapp ">=2.0.0,<3.0.0"
galvaan pin myapp "^1.0"
galvaan pin myapp "~1.2"

# Remove pin
galvaan unpin myapp
```

Pinned apps will only be updated within the constraint. Use `galvaan list` to see active pins.

## Prerelease versions

By default, prerelease versions are skipped. Enable them per-app:

```bash
# When adding
galvaan add owner/repo --asset-pattern "*.rpm" --prerelease

# Override for a single check/update
galvaan check myapp --prerelease
galvaan update myapp --prerelease
```

## Specific version install

Install a specific release version instead of latest:

```bash
galvaan update myapp --version v1.0.24
galvaan update myapp --version 1.0.24
```

## Keeping galvaan itself up to date

Galvaan can track its own releases just like any other app:

```bash
# RPM-based (openSUSE, Fedora)
galvaan add raymens/Galvaan --name galvaan --asset-pattern "*.rpm"

# DEB-based (Debian, Ubuntu)
galvaan add raymens/Galvaan --name galvaan --asset-pattern "*.deb"

# Then update as usual
galvaan update galvaan
```

## Configuration

Config is stored at `~/.config/galvaan/config.toml`. Use `galvaan config path` to show the exact location.

```toml
[settings]
# Auto-approve installs: "always", "no_deps" (default), or "never"
#   always   — always auto-approve (zypper -y)
#   no_deps  — auto-approve only when no new dependencies are needed
#   never    — always prompt for confirmation
auto_approve = "no_deps"

# Default package manager for new apps (used when --package-manager is not passed to `add`)
# Supported: zypper, dnf, apt, pacman
default_package_manager = "zypper"

# Hide package manager output during install
quiet_package_manager = false

# Log file path (omit to disable file logging)
log_file = "~/.local/share/galvaan/galvaan.log"

# Log level: trace, debug, info, warn, error
log_level = "info"

[apps.github-copilot]
repo = "github/app"
asset_pattern = "*-linux-x64.rpm"
package_manager = "zypper"

[apps.beta-tool]
repo = "owner/beta-tool"
asset_pattern = "*.rpm"
package_manager = "zypper"
allow_prerelease = true
version_pin = "1.*"
```

### Managing settings via CLI

```bash
# Show current settings
galvaan config show

# Set auto-approve to always
galvaan config set auto_approve always

# Set default package manager
galvaan config set default_package_manager zypper

# Enable quiet mode
galvaan config set quiet_package_manager true

# Enable file logging
galvaan config set log_file ~/.local/share/galvaan/galvaan.log
galvaan config set log_level debug

# Disable file logging
galvaan config set log_file none
```

## Shell completions

Generate completions for your shell and source them:

```bash
# Bash
galvaan completions bash > ~/.local/share/bash-completion/completions/galvaan

# Zsh
galvaan completions zsh > ~/.local/share/zsh/site-functions/_galvaan

# Fish
galvaan completions fish > ~/.config/fish/completions/galvaan.fish
```

## Building

```bash
cargo build --release
```

## Running tests

```bash
# Unit tests
cargo test

# Integration tests (requires podman or docker + network access)
cargo build --release
cargo test --test container_integration -- --ignored
```

Integration tests build a container for each supported distro, copy in the galvaan binary, and exercise the full CLI inside a real environment. Currently tested:

- openSUSE Tumbleweed (zypper)
- Fedora (dnf)
- Ubuntu (apt)
- Arch Linux (pacman)

To add a new distro, add a `Containerfile.<distro>` in `tests/integration/distros/` and a test function in `tests/container_integration.rs`.

## Roadmap

- [ ] UI (desktop app)
- [x] Configurable default package manager
- [x] Shell completions (bash, zsh, fish, elvish, powershell)
- [x] Multiple package managers (zypper, dnf, apt, pacman)
- [x] Version pinning (exact, wildcard, semver ranges)
- [x] Prerelease version support
- [x] Specific version install
- [ ] Flatpak support
- [ ] Scheduled background update checks
- [ ] GitHub token support for private repos / rate limits
- [ ] Pre/post install hooks
