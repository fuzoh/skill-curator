#!/usr/bin/env bash
# Fast blocking quality gates. CI runs this exact script, so a local pass means
# the same checks pass in CI. The script refuses to run on any Rust release
# other than the one pinned in rust-toolchain.toml, because rustfmt and Clippy
# findings differ between releases.
#
# Prerequisites: the pinned Rust toolchain with rustfmt and clippy, and
# cargo-deny (`cargo install cargo-deny --locked`).
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

run() {
    printf '\n==> %s\n' "$*"
    "$@"
}

# Fails unless `rustc` and `cargo` report the release pinned in
# rust-toolchain.toml. Without rustup the pin is not applied automatically, so
# a system toolchain could silently run different checks than CI.
require_pinned_toolchain() {
    local pinned tool name version
    pinned="$(sed -n 's/^channel[[:space:]]*=[[:space:]]*"\([^"]*\)".*$/\1/p' rust-toolchain.toml)"
    if [[ -z "$pinned" ]]; then
        echo "check.sh: error: no toolchain channel found in rust-toolchain.toml" >&2
        exit 1
    fi
    for tool in rustc cargo; do
        read -r name version _ <<<"$("$tool" --version)"
        if [[ "$name" != "$tool" || "$version" != "$pinned" ]]; then
            echo "check.sh: error: rust-toolchain.toml pins Rust $pinned but \`$tool --version\` reports ${version:-nothing}." >&2
            echo "check.sh: install Rust $pinned (rustup applies the pin automatically) and run again." >&2
            exit 1
        fi
    done
    printf '==> Rust %s (pinned in rust-toolchain.toml)\n' "$pinned"
}

require_pinned_toolchain

run cargo fmt --all --check
run cargo clippy --all-targets --all-features --locked -- -D warnings
run cargo test --all-features --locked
run cargo deny check
