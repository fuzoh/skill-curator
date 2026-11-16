# Lean Rust CLI quality tooling

**Research date / access date:** 2026-11-16 (UTC). **Status:** recommendations, not an accepted quality policy or setup. [Rust quality tooling: tests, mutation, duplication, and lint](https://github.com/fuzoh/skill-curator/issues/28) supplies the settled context: Rust; clap/serde likely; Cargo installation from this source repository; system Git allowed at runtime; no dependency on `npx skills`; no implementation yet. The owner's remaining tooling, lint severity, license policy, supported platforms, MSRV, and check cadence remain decisions.

## Recommendation in brief

Start with **stable rustfmt + default Clippy + Cargo tests**, small CLI integration tests using **assert_cmd + tempfile** (or assert_fs for fixture ergonomics), and **cargo-deny** for dependency/license policy once the owner specifies it. Keep **cargo-mutants** as a slower, isolated, targeted test-effectiveness check; keep **duplication detection advisory** until actual code justifies a gate. Use plain assertions/fixture files first; add **Insta or trycmd**, not both by default, when output-regression maintenance becomes material.

This is a proposed lean baseline, not a decision to enforce warnings-as-errors, select allowed licenses, require mutation scores, or set duplication/coverage/CRAP thresholds. There is no code against which to calibrate such thresholds. Coverage describes execution, not whether assertions catch wrong behavior; cargo-mutants explicitly distinguishes these questions. [M1]

## What Cargo already supplies, and what it does not

`cargo test` compiles and executes unit, integration, and library documentation tests. Rust's default harness runs `#[test]` functions concurrently. Cargo builds binary targets for selected integration tests and exposes `CARGO_BIN_EXE_<name>` so tests can launch them. Tests start in the package root, **not in an isolated filesystem**. `--all-targets` selects libraries, binaries, tests, benches, and examples, but does **not** include doctests; retain plain `cargo test` or a separate `cargo test --doc` when appropriate. `--locked` fails if the project lockfile is missing or needs changes. [C1]

Cargo therefore already supports regression tests: write a test that captures a previously broken behavior. CLI exit/stdout/stderr comparisons, filesystem postconditions, Git fixtures, snapshot review, mutation testing, clone detection, advisory lookup, and license policy need test code or additional tools; they are not a turnkey safety suite supplied by Cargo. This distinction is an inference from the documented harness and the specialized tools below, not a claim that integration testing requires a third-party crate. [C1, T1, T2, T3, M1, D1, S1]

## Candidate commands, prerequisites, and trade-offs

**Examples below are documentation-derived future commands. None was run or installed during this research.** Placeholder paths/binary names are illustrative. Pin actual tool versions and a CI Rust toolchain after an owner decision; do not silently follow moving `latest` releases. Source installation of Cargo tools requires a suitable Rust compiler/Cargo and host linker/toolchain; prebuilt tools can avoid their own compilation requirement. This is separate from the Rust version needed to compile skill-curator. [C2, M2]

### Formatting and lint: use the Rust toolchain components

```sh
rustup component add rustfmt clippy
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked
# Optional strict gate, only if the owner chooses it:
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

- **rustfmt:** stable component, no Node. `--check` reports formatting differences without formatting in place. Stable defaults are sufficient; unstable options require nightly. Macro/comment formatting has limitations. Low relative CI cost: formatting rather than a full test run. [F1]
- **Clippy:** default groups catch correctness, suspicious code, style, complexity, and performance issues. `pedantic` has documented false positives; `restriction` must not be enabled wholesale because lints can contradict one another. Use narrow, explained exceptions rather than every lint category. Clippy needs compiler analysis, so expect build/check cost and toolchain-upgrade churn. `-D warnings` also promotes rustc warnings. Its README now prefers `CARGO_BUILD_WARNINGS=deny` on Cargo 1.97+, but the older command above avoids assuming that version; neither severity policy is selected here. [F2]
- Do not blindly add `--all-features`: Cargo says it activates all features, not that every combination is valid. Choose feature combinations once a manifest exists. Pinning CI Rust is a recommendation to make lint/format changes explicit, not a chosen MSRV. [C1]

### Tests: built-in harness, minimal helpers, optional output snapshots

| Option | Future command / use | Prerequisites and limitations |
|---|---|---|
| Cargo + standard assertions/process APIs | `cargo test --workspace --locked`; targeted `cargo test --test cli` once that target exists | No extra runner. Test authors must implement process assertions, fixture creation, cleanup, and Git isolation. Concurrency makes shared paths/process-global environment changes unsafe test designs. [C1] |
| assert_cmd + tempfile (recommended starting helpers) | Add as **dev-dependencies**; ordinary `cargo test`; `assert_cmd::Command::cargo_bin("<binary>")` locates the binary, then assert success/failure/output | assert_cmd source inspected declares Rust 1.85; its binary discovery is integration-test-specific and reuses the existing target/features/build mode. tempfile supports major desktop/server OSes and declares Rust 1.63 for its usual targets. Temp directories are convenience/isolation of names, **not a security sandbox**. [T1, T2] |
| assert_fs instead of hand-written fixture/assertion helpers | Add as a dev-dependency; `cargo test` | Supplies fixture setup and produced-file assertions. Inspected source declares Rust 1.85; adds helper dependencies. Useful if many fixture trees are needed; not required alongside tempfile merely to obtain a temporary directory. [T3] |
| Plain fixture strings/files vs Insta | Plain `assert_eq!` first. Optional: `cargo install --locked cargo-insta --version 1.49.0`; `cargo test`; `cargo insta review` | Insta compares complex values with stored snapshots and provides review tooling. Library and companion 1.49.0 manifests declare Rust 1.66. cargo-insta is optional for file snapshots but required for inline snapshot updates. Snapshot churn and rubber-stamp acceptance can hide regressions. [T4] |
| trycmd instead of a growing hand-written CLI transcript harness | Add as a dev-dependency; a test calls `trycmd::TestCases::new().case("tests/cmd/*.toml")`; run `cargo test` | Discovers case files, runs commands, verifies results; can test README examples. Inspected workspace declares Rust 1.85. Attractive for many declarative CLI cases, but file/Git safety still needs explicit fixtures/postconditions. [T5] |

Insta's `cargo insta test` **forces snapshot assertions to pass by default** to collect pending changes; do not mistake that review workflow for a strict CI test gate. Use normal `cargo test`, or deliberately configure `--no-force-pass`. Do not auto-accept snapshots in CI. Normalize nondeterministic paths, timestamps, terminal colors, and ordering only where they are not part of the behavior under test. Snapshot approval risk and normalization guidance are recommendations; the force-pass behavior is documented fact. [T4]

Expected CI costs (qualitative, not measured here): Cargo compiles test/dev dependencies; process tests add binary launches; Git fixtures add subprocess/disk work; snapshots add artifact storage and review effort. Prefer local fixture repositories rather than public network fetches in routine tests. No runner upgrade is needed merely to write unit/integration/regression tests. [C1, T1–T5]

### Filesystem/Git safety: proposed test obligations, not tool features

The following matrix is a **project-specific recommendation** derived from `docs/goals.md` (protect unrelated files/local edits), `docs/design-notes.md` (untrusted upstream, traversal/symlink safety), and the Glossary. It is not implemented or validated behavior.

| Area | Minimum useful regressions / assertions |
|---|---|
| CLI contracts | Help/version, invalid flags, invalid Manifest, missing Source/Revision/Git, noninteractive error path; assert exit status and relevant stderr as well as stdout. |
| Write confinement | Reject absolute/traversal paths from Source content; test symlinked Destination ancestors, links pointing outside approved roots, and dangling links; sentinel files outside Destination remain byte-identical after success **and failure**. |
| Preservation | Unrelated files and local edits survive; conflict/refusal leaves previously accepted content unchanged; repeat operation is idempotent. |
| Git fixtures | Per-test local temporary Source repository with explicitly configured identity and two known commits; pin immutable Revision, exercise dirty state/conflicts/missing revision and paths with spaces; assert expected Git state, no accidental checkout changes in user Collection. |
| Failure handling | Missing permissions where supported, interrupted/failed writes via deterministic fault injection, subprocess failure; check coherent Manifest/lock/Destination state, not just an error message. Exact recovery guarantees await the spec. |
| Trust boundaries | Fixture scripts/hooks/instructions write a sentinel if executed; verify normal fetch/inspect/update-review never runs them. Do not use real credentials or execute fetched instructions to construct fixtures. |

Run subprocesses with per-child working directory/environment rather than changing process-global cwd/environment in parallel tests. Remove inherited Git override variables; use temporary HOME/XDG paths, `GIT_CONFIG_NOSYSTEM=1`, an explicit temporary empty file for `GIT_CONFIG_GLOBAL`, and `GIT_TERMINAL_PROMPT=0`. Git documents the configuration overrides and prompt switch; these settings are **not** an OS sandbox and do not by themselves remove every credential helper/hook/network path. Fixture-specific Git config and no-network isolation are still needed. [G1, C1]

Recommend the owner's actual workstation/server OSes first, then any promised portable OSes. Symlink privileges, file permissions, path handling, and rename behavior need platform-specific test cases; a successful Linux run cannot establish Windows safety. Keep filesystem-changing integration and mutation tests in a disposable environment without host home/SSH/credentials mounted, and preferably without network. Temporary directories alone cannot constrain buggy deletion code. cargo-mutants specifically warns that mutated deletion paths can damage unintended directories. [M3]

### Mutation: cargo-mutants as a slower, isolated effectiveness check

```sh
cargo install --locked cargo-mutants --version 27.1.0
cargo mutants -f src/<safety-module>.rs
# Later, when affordable and isolated:
cargo mutants
```

**Why this candidate:** works on existing Cargo test suites without instrumenting each production function; v27.1.0 declares Rust **1.88** to build. Its upstream CI explicitly includes Linux, macOS, Windows and Rust 1.88/stable/nightly. Installation and tested upstream platforms do not prove compatibility with this future CLI. [M1, M2]

**Costs/limitations:** each viable mutant involves an incremental build and test run, so cost grows roughly with mutants × (build + selected tests), plus baseline/copy overhead. Parallel workers increase CPU/RAM/disk demand. Baseline must pass reliably in a copied tree; default copies exclude `.git`, which matters if tests accidentally depend on the checkout instead of creating Git fixtures. Avoid `--in-place`. [M4, M5]

A missed mutant may reveal a missing assertion, or be semantically equivalent to the original code; unviable mutants do not demonstrate ineffective tests. Hangs/timeouts and platform-conditional/generated code require review rather than a simplistic score. The manual documents test timeouts and configurable build timeouts. Do not make an arbitrary mutation percentage or “all mutants caught” a default gate. Start on path validation, overwrite protection, Revision selection, and update/conflict logic; add a regression for each real gap. [M6, M7]

**Maintenance risk:** release v27.1.0 was published 2026-06-02; inspected HEAD has a 2026-10-19 commit. README explicitly calls this a semi-actively-maintained spare-time project as of August 2026 with higher response latency. It also illustrates documentation drift: README supports nextest whereas the limitations chapter still describes nextest as future work. The recommendation uses plain Cargo tests, not an assumed runner integration. [M1, E1]

**Alternative screened, not recommended:** Mutagen documents single-compilation/runtime-selected mutation and `cargo install cargo-mutagen` / `cargo mutagen`, but its README labels it an architecture preview, requires nightly, and says its documented 0.2.0 is unreleased. Latest inspected commit is 2022-07-29. Annotation/instrumentation and maintenance uncertainty make it a poor lean default despite potentially lower repeated-build cost. This is not proof it cannot work; no experiment was performed. [M8]

### Duplication: optional advisory jscpd, not a refactoring mandate

**Important version correction:** current jscpd is not necessarily a Node runtime dependency. Released **v5.4.3** documents a Rust self-contained binary; the older v4 engine needs Node.js 20+. v5 has Cargo and prebuilt installation routes. Inspected v5.4.3 workspace declares Rust **1.97** to build, a potentially much higher developer-tool requirement than the application MSRV. Prefer evaluating a verified release binary before making developers build that dependency graph. [D1, D2]

```sh
# Optional Rust-native development-tool installation:
cargo install --locked jscpd --version 5.4.3
# Advisory scan; start with production code rather than fixtures/generated files:
jscpd src --format rust --reporters console
# Older Node-only alternative, explicitly development/CI only:
npx --yes jscpd@4 src --format rust
```

`rust` / `.rs` is listed in the released formats file. The detector matches token sequences, not whether two functions should share an abstraction; repeated error handling, setup code, and intentionally independent safety checks may be legitimate. Defaults (50 tokens / 5 lines) are **tool defaults, not recommended project thresholds**. v5 offers baseline/new-clone gating if later justified; initially review findings without enforcing a percentage. Scanning a small `src` tree is expected to be cheaper than repeated mutation builds, but no timing or cross-tool benchmark was performed. Do not carry vendor benchmark numbers into a speed ranking. [D1, D3]

**Trade-off / alternative:** for a tiny codebase, manual duplication review plus Clippy's localized simplification suggestions is cheaper operationally, but is not a whole-tree clone detector. v4 is a fallback only when Node tooling is deliberately acceptable; its maintenance policy says fixes on `master-v4`, new features on v5. Neither Node nor npm/npx nor jscpd belongs in skill-curator runtime/install scripts. `npx jscpd` is unrelated to `npx skills`. Avoid optional semantic/embedding/API features: they are unnecessary for this check and can add model download or external processing. [D1, D2, F2]

### Dependencies/licenses: prefer one consolidated checker

```sh
cargo install --locked cargo-deny --version 0.21.0
# A future setup step only; requires owner review, not executed here:
cargo deny init
cargo deny check advisories licenses bans sources
# Narrower alternative (advisories only):
cargo install --locked cargo-audit --version 0.23.0
cargo audit
```

**cargo-deny:** covers RustSec advisories, license acceptability, banned/duplicate crate versions, and allowed sources in one tool. 0.21.0 manifest declares Rust **1.89.0**; README badge says 1.88.0, so prefer the versioned manifest for the build prerequisite. Config must express the owner's license/source/exception policy; `init` is not policy acceptance. [S1, S2]

Its license checker handles SPDX expressions, confidence-based license identification, and hash-pinned clarifications when metadata is incomplete. Unlisted licenses are denied. Current docs say dev-dependency licenses are excluded by default (`include-dev = false`), build-dependency licenses included (`include-build = true`): explicitly decide whether developer/test dependencies are part of this repository's audit scope. Missing/misclassified metadata can require manual review; multiple versions may be legitimate, so bans warnings are not automatic proof of a problem. [S3, S1]

**cargo-audit:** narrower RustSec advisory scan, useful if the owner wants security checking before broader policy. It does **not replace license checking**. v0.23.0 manifest requires Rust **1.88**, despite README saying 1.74. Prefer the versioned manifest. Its README warns that a missing lockfile can trigger Cargo resolution; run it only on the trusted tool source, not arbitrary fetched Skill repositories. Do not use `cargo audit fix` as an unreviewed CI repair step. [S4, S5]

**Limitations/cost:** advisory results depend on database freshness and recorded advisories, not proof of no vulnerability or exploitability. cargo-audit documents exceptions for unused vulnerable APIs/features, so review scope rather than blindly ignore. Advisory refresh introduces network/database availability and cache management; license checking needs dependency source/metadata, but not per-mutant recompilation. Check on dependency-changing work and periodically even without code changes because advisories can change. Running both deny and audit by default duplicates substantial advisory checking; choose one unless there is a demonstrated gap. Costs and cadence are recommendations/inferences, not measured timings. [S1, S4]

**MIT context:** deny/audit/assert helpers have MIT-or-Apache-2.0 licensing; cargo-mutants and jscpd are MIT; Insta is Apache-2.0. These facts do not establish the compliance of a nonexistent resolved application dependency graph. Keep test libraries in `[dev-dependencies]`, separate installed checker tools from application dependencies, and preserve required notices if distributing their code/binaries. Cargo documents that dev-dependencies are for tests/examples/benchmarks, not ordinary package builds or propagated dependencies. Owner-approved SPDX allowlists, exceptions, notice obligations and any legal interpretation remain separate decisions. cargo-deny explicitly disclaims legal advice. Cargo checks also do **not** audit licenses of downloaded third-party Skill content; that requires the product's separate Source/provenance/license workflow. [C3, S1, T1–T5, M2, D1]

## Maintenance evidence (dated observations, not durability guarantees)

Inspected GitHub API commit/release records on 2026-11-16. A recent commit may be automation; dates establish activity, not correctness, responsiveness, or independent benchmark quality. HEAD source versions below are **not asserted to be published crate versions**. Versioned manifests take precedence over stale README badges.

| Tool | Observed record |
|---|---|
| rustfmt | [HEAD commit 2026-11-16](https://github.com/rust-lang/rustfmt/commit/8b050cd71dead18f3fc2bae8eadac29fe2b4c39c); standalone latest release API reports v1.6.0 from 2023-07-01, which alone is misleading for a toolchain-distributed component. |
| Clippy | [HEAD commit 2026-11-15](https://github.com/rust-lang/rust-clippy/commit/c96aca17f5dbe90c988e3aa7e922203f4f40af97); installed with rustup component, not a separately selected checker crate release. |
| assert_cmd / assert_fs | [assert_cmd HEAD 2026-11-16](https://github.com/assert-rs/assert_cmd/commit/97323eda8e08525c20dcf62ab743020b985a675d), manifest 2.2.2; [assert_fs HEAD 2026-11-16](https://github.com/assert-rs/assert_fs/commit/af7e6514a602d1fe29d3da47ba2d523618583c0d), manifest 1.1.4. |
| tempfile / trycmd | [tempfile HEAD 2026-10-30](https://github.com/Stebalien/tempfile/commit/5fe9b1280fba8421203ab49b421dae009dd0b7b7); [trycmd HEAD 2026-11-16](https://github.com/assert-rs/trycmd/commit/62b0cda401c0d8a49bedb4f1f727eb762775f354), crate manifest 1.2.1. |
| Insta | [1.49.0 release, 2026-10-03](https://github.com/mitsuhiko/insta/releases/tag/1.49.0). |
| cargo-mutants | [v27.1.0 release, 2026-06-02](https://github.com/sourcefrog/cargo-mutants/releases/tag/v27.1.0); [HEAD 2026-10-19](https://github.com/sourcefrog/cargo-mutants/commit/6c720ae82a1784b84cfa06066f7b0d59ed1798d0); maintainer caveat above. |
| jscpd | [v5.4.3 release, 2026-11-08](https://github.com/kucherenko/jscpd/releases/tag/v5.4.3). |
| cargo-deny / cargo-audit | [deny 0.21.0 release, 2026-11-16](https://github.com/EmbarkStudios/cargo-deny/releases/tag/0.21.0); [audit 0.23.0 release, 2026-11-10](https://github.com/rustsec/rustsec/releases/tag/cargo-audit/v0.23.0). |
| Mutagen (screened alternative) | [HEAD 2022-07-29](https://github.com/llogiq/mutagen/commit/a6377c4c3f360afeb7a287c1c17e4b69456d5f53), preview/unreleased caveat in README. |

## Proposed split and owner's next decisions

1. **Fast normal checks:** rustfmt check; default Clippy; unit/CLI/filesystem/Git regression tests; dependency/license check after policy review. Consider cache/install cost before putting every tool installation in every job.
2. **Slower/optional:** isolated targeted cargo-mutants initially on-demand, broader runs later on an owner-selected schedule; advisory jscpd scans once there is enough source; add snapshots only for genuinely useful stable contracts.
3. **Owner decisions still needed:** supported OSes and Rust/MSRV; strictness and upgrade cadence; whether any optional check gates CI; test scope and isolation budget; license allowlist/dev/build scope and reviewed exceptions; whether snapshots justify another helper. No numeric coverage/mutation/duplication/CRAP target is proposed.
4. **First implementation follow-up (not performed):** build a small representative CLI/fixture experiment, measure cold/warm CI duration and tool-install time, verify supported OSes, then choose gates from observed signal/noise. This research deliberately stops before implementation.

## Inspection scope and access gaps

Read the requested research skill, project AGENTS.md, issue-tracker/domain guidance, goals/design notes, GLOSSARY, and [Rust quality tooling: tests, mutation, duplication, and lint](https://github.com/fuzoh/skill-curator/issues/28) including its existing comment. Repository base: `b79bc1c`. Project requirement references are working-tree context, not evidence of implemented Rust behavior.

The caller attempted Context7 with `ctx7 library rust` and received a monthly-quota error. No Context7 lookup succeeded; the delegated investigation used official docs/source and authenticated read-only GitHub API retrieval directly. No deep-research expansion was needed for this bounded official-tooling comparison. Git's documentation website returned HTTP 403; the official Git source documentation was inspected instead. Some guessed documentation paths returned 404 and were replaced by the retrieved official passages cited below.

No Rust tool was installed, no test/build/benchmark ran, and no setup/config/code was implemented. Commands and requirements are documented, not locally verified. No paid service was used; no specific CI duration, hosted-runner bill, or third-party dependency compatibility can be established without implementation and a resolved graph. Tool licenses permit source inspection but are not a substitute for distribution/legal review.

## Primary sources

Links to repositories use recorded commits where practical; release-specific facts use explicit release tags. Web documentation is unversioned and was accessed 2026-11-16. Maintenance publication dates above came from GitHub release API records (not inferred from tag names).

- **C1:** [Cargo test reference — execution, binary discovery, targets, features, lockfile](https://doc.rust-lang.org/cargo/commands/cargo-test.html).
- **C2:** [Cargo install reference — source/path/git, versions and toolchain/build](https://doc.rust-lang.org/cargo/commands/cargo-install.html).
- **C3:** [Cargo development dependencies](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#development-dependencies).
- **F1:** [rustfmt README — stable component, check, limitations, nightly settings](https://github.com/rust-lang/rustfmt/blob/8b050cd71dead18f3fc2bae8eadac29fe2b4c39c/README.md).
- **F2:** [Clippy README — lint groups, false positives, installation, warning levels](https://github.com/rust-lang/rust-clippy/blob/c96aca17f5dbe90c988e3aa7e922203f4f40af97/README.md).
- **T1:** assert_cmd [README](https://github.com/assert-rs/assert_cmd/blob/97323eda8e08525c20dcf62ab743020b985a675d/README.md), [binary lookup implementation/docs](https://github.com/assert-rs/assert_cmd/blob/97323eda8e08525c20dcf62ab743020b985a675d/src/cargo.rs), [manifest](https://github.com/assert-rs/assert_cmd/blob/97323eda8e08525c20dcf62ab743020b985a675d/Cargo.toml).
- **T2:** [tempfile README — platform support and MSRV](https://github.com/Stebalien/tempfile/blob/5fe9b1280fba8421203ab49b421dae009dd0b7b7/README.md).
- **T3:** assert_fs [README](https://github.com/assert-rs/assert_fs/blob/af7e6514a602d1fe29d3da47ba2d523618583c0d/README.md), [manifest](https://github.com/assert-rs/assert_fs/blob/af7e6514a602d1fe29d3da47ba2d523618583c0d/Cargo.toml).
- **T4:** Insta [README](https://github.com/mitsuhiko/insta/blob/0e17c7a6ed65b73def1101813987a053269ef4a5/README.md), [review/test commands and force-pass caveat](https://insta.rs/docs/cli/), [1.49.0 library manifest](https://github.com/mitsuhiko/insta/blob/1.49.0/insta/Cargo.toml), [companion manifest](https://github.com/mitsuhiko/insta/blob/1.49.0/cargo-insta/Cargo.toml).
- **T5:** trycmd [README/example](https://github.com/assert-rs/trycmd/blob/62b0cda401c0d8a49bedb4f1f727eb762775f354/crates/trycmd/README.md), [workspace MSRV/license](https://github.com/assert-rs/trycmd/blob/62b0cda401c0d8a49bedb4f1f727eb762775f354/Cargo.toml), [crate manifest](https://github.com/assert-rs/trycmd/blob/62b0cda401c0d8a49bedb4f1f727eb762775f354/crates/trycmd/Cargo.toml).
- **G1:** [Git official source documentation — configuration and prompt environment](https://github.com/git/git/blob/7fb6055e678f800320d8ab8cd8f201d9493d1ba0/Documentation/git.adoc).
- **M1:** [cargo-mutants README — commands, prerequisites, effectiveness vs coverage, August 2026 status](https://github.com/sourcefrog/cargo-mutants/blob/6c720ae82a1784b84cfa06066f7b0d59ed1798d0/README.md).
- **M2:** cargo-mutants v27.1.0 [manifest](https://github.com/sourcefrog/cargo-mutants/blob/v27.1.0/Cargo.toml), [platform/toolchain CI](https://github.com/sourcefrog/cargo-mutants/blob/v27.1.0/.github/workflows/tests.yml), [installation](https://github.com/sourcefrog/cargo-mutants/blob/v27.1.0/book/src/installation.md).
- **M3:** [cargo-mutants safety cautions](https://mutants.rs/cautions.html).
- **M4:** [cargo-mutants performance costs](https://mutants.rs/performance.html).
- **M5:** [cargo-mutants copying, VCS exclusions, in-place behavior](https://mutants.rs/build-dirs.html).
- **M6:** [cargo-mutants outcomes, equivalent mutants, prioritization](https://github.com/sourcefrog/cargo-mutants/blob/v27.1.0/book/src/using-results.md).
- **M7:** [timeouts](https://mutants.rs/timeouts.html), [limitations and stale nextest paragraph](https://github.com/sourcefrog/cargo-mutants/blob/v27.1.0/book/src/limitations.md).
- **M8:** [Mutagen preview README, commands, nightly and safety restrictions](https://github.com/llogiq/mutagen/blob/a6377c4c3f360afeb7a287c1c17e4b69456d5f53/README.md).
- **E1:** [GitHub cargo-mutants release record](https://api.github.com/repos/sourcefrog/cargo-mutants/releases/tags/v27.1.0).
- **D1:** jscpd v5.4.3 [installation/platforms/options/defaults](https://github.com/kucherenko/jscpd/blob/v5.4.3/docs/rust.md), [workspace build requirement/license](https://github.com/kucherenko/jscpd/blob/v5.4.3/rust/Cargo.toml).
- **D2:** [jscpd v4 vs v5 — Node requirements and maintenance policy](https://github.com/kucherenko/jscpd/blob/v5.4.3/README-v4.md).
- **D3:** [released jscpd formats, including Rust](https://github.com/kucherenko/jscpd/blob/v5.4.3/FORMATS.md).
- **S1:** [cargo-deny README — commands, check types, licensing, legal disclaimer](https://github.com/EmbarkStudios/cargo-deny/blob/4b1f2002a11077f25437c2983f0a1a5439a886f4/README.md).
- **S2:** [cargo-deny 0.21.0 versioned manifest](https://github.com/EmbarkStudios/cargo-deny/blob/0.21.0/Cargo.toml).
- **S3:** [cargo-deny license policy — dev/build scopes, SPDX, confidence and clarifications](https://github.com/EmbarkStudios/cargo-deny/blob/4b1f2002a11077f25437c2983f0a1a5439a886f4/docs/src/checks/licenses/cfg.md).
- **S4:** [cargo-audit README — advisory scope, lockfile/trust warning, fixes, exceptions](https://github.com/rustsec/rustsec/blob/682f3a519d1d14ecd07b49f135e992fe39761545/cargo-audit/README.md).
- **S5:** [cargo-audit 0.23.0 versioned manifest, Rust 1.88](https://github.com/rustsec/rustsec/blob/cargo-audit/v0.23.0/cargo-audit/Cargo.toml).
