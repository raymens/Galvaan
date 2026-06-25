#!/usr/bin/env bash
# Integration test script that runs inside a distro container.
# Exit code 0 = all tests passed.
# Each test function prints PASS/FAIL and sets the global rc on failure.
#
# Environment:
#   GALVAAN_TEST_PM  — the package manager expected on this distro (zypper, dnf, apt, pacman)
set -u

PM="${GALVAAN_TEST_PM:-zypper}"
RC=0

pass() { echo "  PASS: $1"; }
fail() { echo "  FAIL: $1"; RC=1; }

run_test() {
    local name="$1"
    shift
    if "$@"; then
        pass "$name"
    else
        fail "$name"
    fi
}

# ─── Helpers ───────────────────────────────────────────────────────────────────

assert_exit_0() { "$@"; }
assert_exit_nonzero() { ! "$@"; }
assert_output_contains() {
    local expected="$1"; shift
    local output
    output=$("$@" 2>&1)
    if echo "$output" | grep -qF "$expected"; then
        return 0
    else
        echo "    Expected output to contain: $expected"
        echo "    Got: $output"
        return 1
    fi
}

# Pick the right asset pattern per distro PM
case "$PM" in
    zypper|dnf) ASSET_PATTERN="*-linux-x64.rpm" ;;
    apt)        ASSET_PATTERN="*-linux-x64.deb" ;;
    pacman)     ASSET_PATTERN="*-linux-x64.tar.gz" ;;
    *)          ASSET_PATTERN="*-linux-x64.rpm" ;;
esac

# ─── CLI basics ────────────────────────────────────────────────────────────────

echo "=== CLI basics ==="

run_test "galvaan --version" \
    assert_output_contains "galvaan" galvaan --version

run_test "galvaan --help" \
    assert_output_contains "GitHub releases" galvaan --help

run_test "galvaan list (empty)" \
    assert_output_contains "No apps tracked" galvaan list

# ─── Config management ────────────────────────────────────────────────────────

echo "=== Config management ==="

run_test "config path" \
    assert_output_contains "galvaan/config.toml" galvaan config path

run_test "config show (defaults)" \
    assert_output_contains "auto_approve = no_deps" galvaan config show

# Set default_package_manager to match this distro
galvaan config set default_package_manager "$PM" >/dev/null 2>&1

run_test "config show default_package_manager" \
    assert_output_contains "default_package_manager = $PM" galvaan config show

run_test "config set auto_approve always" \
    assert_exit_0 galvaan config set auto_approve always

run_test "config set persisted" \
    assert_output_contains "auto_approve = always" galvaan config show

run_test "config set quiet_package_manager" \
    assert_exit_0 galvaan config set quiet_package_manager true

run_test "config set log_level" \
    assert_exit_0 galvaan config set log_level debug

run_test "config set invalid key" \
    assert_exit_nonzero galvaan config set nonexistent_key value

# Reset for further tests
galvaan config set auto_approve no_deps >/dev/null 2>&1
galvaan config set quiet_package_manager false >/dev/null 2>&1

# ─── Add / List / Remove ──────────────────────────────────────────────────────

echo "=== Add / List / Remove ==="

run_test "add app" \
    assert_exit_0 galvaan add github/app --name github-copilot --asset-pattern "$ASSET_PATTERN"

run_test "list shows added app" \
    assert_output_contains "github-copilot" galvaan list

run_test "list shows repo" \
    assert_output_contains "github/app" galvaan list

run_test "app uses correct package manager" \
    assert_output_contains "$PM" galvaan list

run_test "add second app" \
    assert_exit_0 galvaan add cli/cli --name gh-cli --asset-pattern "$ASSET_PATTERN"

run_test "list shows both apps" \
    assert_output_contains "gh-cli" galvaan list

run_test "remove app" \
    assert_exit_0 galvaan remove gh-cli

run_test "removed app is gone" \
    assert_exit_nonzero assert_output_contains "gh-cli" galvaan list

# ─── Add with explicit --package-manager ───────────────────────────────────────

echo "=== Explicit --package-manager ==="

run_test "add with explicit pm" \
    assert_exit_0 galvaan add owner/test-explicit --name test-explicit \
        --asset-pattern "*.rpm" --package-manager "$PM"

run_test "explicit pm shown in list" \
    assert_output_contains "$PM" galvaan list

galvaan remove test-explicit >/dev/null 2>&1

# ─── Package manager detection ────────────────────────────────────────────────

echo "=== Package manager ($PM) ==="

run_test "$PM is available" \
    assert_exit_0 which "$PM"

case "$PM" in
    zypper|dnf)
        run_test "rpm is available" \
            assert_exit_0 which rpm
        ;;
    apt)
        run_test "dpkg is available" \
            assert_exit_0 which dpkg
        ;;
    pacman)
        run_test "pacman is available" \
            assert_exit_0 which pacman
        ;;
esac

# ─── Check (network) ──────────────────────────────────────────────────────────

echo "=== Check for updates ==="

# This hits the GitHub API — may fail if rate-limited, so we tolerate errors
check_output=$(galvaan check github-copilot 2>&1) || true
if echo "$check_output" | grep -qE "(up to date|update available)"; then
    pass "check returns version info"
elif echo "$check_output" | grep -qF "error"; then
    echo "  SKIP: check returned an error (likely rate-limited): $check_output"
else
    fail "check returned unexpected output: $check_output"
fi

# ─── Completions ───────────────────────────────────────────────────────────────

echo "=== Shell completions ==="

run_test "completions bash" \
    assert_output_contains "_galvaan" galvaan completions bash

run_test "completions zsh" \
    assert_output_contains "#compdef galvaan" galvaan completions zsh

run_test "completions fish" \
    assert_output_contains "galvaan" galvaan completions fish

# ─── Config file content ──────────────────────────────────────────────────────

echo "=== Config file integrity ==="

config_file=$(galvaan config path)
if [ -f "$config_file" ]; then
    run_test "config file exists" true
    run_test "config file contains [apps]" \
        grep -q '\[apps' "$config_file"
    run_test "config file contains github-copilot" \
        grep -q 'github-copilot' "$config_file"
    run_test "config file contains package_manager" \
        grep -q "package_manager" "$config_file"
else
    fail "config file does not exist at $config_file"
fi

# ─── Done ──────────────────────────────────────────────────────────────────────

echo ""
if [ $RC -eq 0 ]; then
    echo "All integration tests passed! (distro PM: $PM)"
else
    echo "Some integration tests FAILED (distro PM: $PM)"
fi
exit $RC
