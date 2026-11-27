# Contributing

## Toolchain

`rust-toolchain.toml` pins the Rust release used for every check. With
rustup, `cargo` selects it automatically inside this repository. Without
rustup (for example a distribution package), install that exact release:
`scripts/check.sh` refuses to run when `rustc` or `cargo` reports another
release, because rustfmt and Clippy findings differ between releases. Bump it
deliberately in its own commit and fix any new rustfmt or Clippy findings in
that commit.

System `git` is the only runtime prerequisite of `skill-curator` itself.

## Blocking checks

Run before every push:

```sh
./scripts/check.sh
```

It runs, in order, and stops at the first failure:

1. `cargo fmt --all --check`
2. `cargo clippy --all-targets --all-features --locked -- -D warnings`
3. `cargo test --all-features --locked`
4. `cargo deny check`

The CI workflow (`.github/workflows/ci.yml`) runs this same script on every
push to `main` and every pull request, so a local pass means the same gates
pass in CI. Install cargo-deny once with `cargo install cargo-deny --locked`.

Never make a check pass by weakening it: do not delete or loosen tests or
assertions, relax `Cargo.toml` lints, `clippy.toml`, or `deny.toml`, or skip a
step.

## Lint policy

Configured in `Cargo.toml` (`[lints]`) and `clippy.toml`:

- All Clippy default groups, with warnings promoted to errors.
- `unwrap_used` and `expect_used` are denied in production code: recoverable
  failures return errors. Tests are exempt through `allow-unwrap-in-tests` and
  `allow-expect-in-tests`, which cover `#[test]` functions and `#[cfg(test)]`
  modules only; integration-test helpers return `Result` instead.
- `unsafe_code` is forbidden.
- Whole `pedantic`, `restriction`, `nursery`, or `cargo` groups are never
  enabled. Add individual lints, each with a comment explaining why.

### Local suppressions

A local `#[allow(...)]` or `#[expect(...)]` is acceptable only when the lint
is a false positive or the flagged pattern is the clearest correct code. It
must:

- sit on the narrowest item or expression, never at crate or module level;
- prefer `#[expect(...)]`, so the suppression fails once it is obsolete;
- carry a `reason = "..."` stating the justification. Clippy enforces this
  through `allow_attributes_without_reason`.

Suppressing `unwrap_used` or `expect_used` in production code is not an
acceptable justification for avoiding error handling.

## Tests

- Unit tests live next to the code in `#[cfg(test)]` modules.
- Integration tests in `tests/` drive the built binary with `assert_cmd` and
  isolate every run in a `tempfile` directory, with `HOME` and the XDG
  variables pointing inside it. Tests never touch the network, the user's real
  home or configuration, or real Sources; Git scenarios use local fixture
  repositories created by the test.
- `tests/quality_harness.rs` guards this harness configuration against
  accidental weakening.

## Dependency policy (`deny.toml`)

`cargo deny check` audits the tool's own Rust dependency graph, including
dev and build dependencies:

- **Advisories**: any RustSec advisory fails, including unmaintained and
  unsound crates and yanked versions. An ignore entry needs a reason and an
  expiry date.
- **Licenses**: only permissive licenses compatible with distributing an
  MIT-licensed binary (`MIT`, `Apache-2.0`, `Apache-2.0 WITH LLVM-exception`,
  `Unicode-3.0`). Adding a license or exception is a reviewed policy change.
- **Bans**: no wildcard version requirements; embedded Git libraries (`git2`,
  `libgit2-sys`, `gix`) and HTTP clients are banned because Sources are
  fetched with system `git` and the tool never calls a model API directly.
- **Sources**: crates.io only.

This policy does not cover the licenses of downloaded Skills. Those are
third-party content tracked by the tool itself in `lock.toml` and
`ATTRIBUTION.md`.

## Consultative checks

These checks inform review and never block a pull request. The
`Consultative checks` workflow (`.github/workflows/consultative.yml`) runs
them weekly and on manual dispatch.

### Mutation testing (cargo-mutants)

```sh
cargo install cargo-mutants --locked --version 27.1.0
cargo mutants
```

`.cargo/mutants.toml` restricts mutation to safety-critical modules: path
validation, overwrite protection, Revision selection, and conflict handling.
When you add or move such a module, list its files there in the same change.
The workflow fails when the configured globs select no mutant, because
cargo-mutants would otherwise report success without testing anything; that
failure still blocks no pull request.

Run it only in a disposable environment (a container or CI runner) without
personal data or credentials mounted: a mutated deletion path can escape a
temporary directory. Review each missed mutant: add a regression test, or
record why the mutant is equivalent. There is no score threshold.

### Duplicate code (jscpd v5)

```sh
cargo install jscpd --locked --version 5.4.3
jscpd --config .jscpd.json
```

jscpd v5 is a self-contained Rust binary used as a development-only tool. It
is never a runtime or build dependency of `skill-curator`, and no Node or Bun
installation is involved. `.jscpd.json` keeps jscpd's default detection
settings (not calibrated project thresholds) and sets no failure threshold:
findings are advisory. Repeated safety checks or test setup can be legitimate
duplication.
