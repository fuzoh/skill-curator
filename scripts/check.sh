#!/usr/bin/env bash
# Fast blocking quality gates. CI runs this exact script, so a local pass means
# the same checks pass in CI (on the toolchain pinned in rust-toolchain.toml).
#
# Prerequisites: the pinned Rust toolchain with rustfmt and clippy, and
# cargo-deny (`cargo install cargo-deny --locked`).
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

run() {
    printf '\n==> %s\n' "$*"
    "$@"
}

run cargo fmt --all --check
run cargo clippy --all-targets --all-features --locked -- -D warnings
run cargo test --all-features --locked
run cargo deny check
