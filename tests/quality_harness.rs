//! Regression guards for the quality harness configuration: the lint policy,
//! the cargo-deny policy, and the parity between local and CI checks.

use std::error::Error;
use std::fs;
use std::path::Path;

use toml::Table;

type TestResult = Result<(), Box<dyn Error>>;

fn repo_file(relative: &str) -> Result<String, Box<dyn Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(&path).map_err(|err| format!("{}: {err}", path.display()).into())
}

fn repo_toml(relative: &str) -> Result<Table, Box<dyn Error>> {
    Ok(repo_file(relative)?.parse::<Table>()?)
}

fn lookup<'a>(table: &'a Table, dotted: &str) -> Option<&'a toml::Value> {
    let mut keys = dotted.split('.');
    let mut value = table.get(keys.next()?)?;
    for key in keys {
        value = value.get(key)?;
    }
    Some(value)
}

fn lookup_str<'a>(table: &'a Table, dotted: &str) -> Option<&'a str> {
    lookup(table, dotted).and_then(toml::Value::as_str)
}

#[test]
fn production_code_denies_unwrap_and_expect() -> TestResult {
    let manifest = repo_toml("Cargo.toml")?;
    assert_eq!(
        lookup_str(&manifest, "lints.clippy.unwrap_used"),
        Some("deny")
    );
    assert_eq!(
        lookup_str(&manifest, "lints.clippy.expect_used"),
        Some("deny")
    );
    assert_eq!(
        lookup_str(&manifest, "lints.clippy.allow_attributes_without_reason"),
        Some("deny")
    );
    Ok(())
}

#[test]
fn tests_keep_a_scoped_unwrap_and_expect_exception() -> TestResult {
    let clippy = repo_toml("clippy.toml")?;
    assert_eq!(
        lookup(&clippy, "allow-unwrap-in-tests").and_then(toml::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        lookup(&clippy, "allow-expect-in-tests").and_then(toml::Value::as_bool),
        Some(true)
    );
    Ok(())
}

#[test]
fn no_blanket_clippy_groups_are_enabled() -> TestResult {
    let manifest = repo_toml("Cargo.toml")?;
    let clippy = lookup(&manifest, "lints.clippy")
        .and_then(toml::Value::as_table)
        .ok_or("missing [lints.clippy]")?;
    for group in ["pedantic", "restriction", "nursery", "cargo"] {
        assert!(
            !clippy.contains_key(group),
            "clippy::{group} must not be enabled wholesale"
        );
    }
    Ok(())
}

#[test]
fn toolchain_is_pinned_to_an_exact_release() -> TestResult {
    let toolchain = repo_toml("rust-toolchain.toml")?;
    let channel = lookup_str(&toolchain, "toolchain.channel").ok_or("missing channel")?;
    let parts = channel.split('.').collect::<Vec<_>>();
    assert_eq!(
        parts.len(),
        3,
        "channel {channel:?} is not MAJOR.MINOR.PATCH"
    );
    assert!(
        parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())),
        "channel {channel:?} is not an exact release"
    );
    Ok(())
}

#[test]
fn deny_policy_is_explicit() -> TestResult {
    let deny = repo_toml("deny.toml")?;

    let allowed = lookup(&deny, "licenses.allow")
        .and_then(toml::Value::as_array)
        .ok_or("missing licenses.allow")?;
    assert!(!allowed.is_empty());
    for license in allowed {
        let license = license.as_str().ok_or("license must be a string")?;
        assert!(
            !license.contains("GPL"),
            "copyleft license {license} in allow list"
        );
    }

    assert_eq!(lookup_str(&deny, "advisories.unmaintained"), Some("all"));
    assert_eq!(lookup_str(&deny, "advisories.unsound"), Some("all"));
    assert_eq!(lookup_str(&deny, "bans.wildcards"), Some("deny"));
    assert_eq!(lookup_str(&deny, "sources.unknown-registry"), Some("deny"));
    assert_eq!(lookup_str(&deny, "sources.unknown-git"), Some("deny"));

    let banned = lookup(&deny, "bans.deny")
        .and_then(toml::Value::as_array)
        .ok_or("missing bans.deny")?
        .iter()
        .filter_map(|entry| entry.get("crate").and_then(toml::Value::as_str))
        .collect::<Vec<_>>();
    for embedded_git in ["git2", "gix"] {
        assert!(
            banned.contains(&embedded_git),
            "{embedded_git} is not banned"
        );
    }
    Ok(())
}

#[test]
fn ci_runs_the_local_check_entrypoint() -> TestResult {
    let check = repo_file("scripts/check.sh")?;
    for command in [
        "cargo fmt --all --check",
        "cargo clippy --all-targets --all-features --locked -- -D warnings",
        "cargo test --all-features --locked",
        "cargo deny check",
    ] {
        assert!(
            check.contains(command),
            "scripts/check.sh lacks `{command}`"
        );
    }

    let ci = repo_file(".github/workflows/ci.yml")?;
    assert!(ci.contains("run: ./scripts/check.sh"));
    assert!(ci.contains("\n  pull_request:"));
    assert!(ci.contains("\n  push:"));
    assert!(
        ci.contains("cancel-in-progress: ${{ github.event_name == 'pull_request' }}"),
        "only superseded pull-request runs may be cancelled, never runs on main"
    );
    Ok(())
}

/// Stand-ins for `rustc` and `cargo`: `--version` reports `$FAKE_RUST_VERSION`,
/// any other `cargo` invocation is appended to `$FAKE_CARGO_LOG`.
#[cfg(unix)]
fn fake_rust_toolchain(bin: &Path) -> TestResult {
    use std::os::unix::fs::PermissionsExt;

    let scripts = [
        (
            "rustc",
            "#!/bin/sh\necho \"rustc $FAKE_RUST_VERSION (fake 2000-01-01)\"\n",
        ),
        (
            "cargo",
            "#!/bin/sh\n\
             if [ \"$1\" = --version ]; then\n  \
               echo \"cargo $FAKE_RUST_VERSION (fake 2000-01-01)\"\n\
             else\n  \
               echo \"$*\" >> \"$FAKE_CARGO_LOG\"\n\
             fi\n",
        ),
    ];
    for (name, body) in scripts {
        let path = bin.join(name);
        fs::write(&path, body)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// Runs `scripts/check.sh` with fake `rustc` and `cargo` reporting `version`,
/// returning the command and the path of the cargo invocation log.
#[cfg(unix)]
fn check_script_with_rust(
    sandbox: &Path,
    version: &str,
) -> Result<(assert_cmd::Command, std::path::PathBuf), Box<dyn Error>> {
    let bin = sandbox.join("bin");
    fs::create_dir(&bin)?;
    fake_rust_toolchain(&bin)?;
    let log = sandbox.join("cargo.log");
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))?;

    let mut cmd = assert_cmd::Command::new("bash");
    cmd.arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/check.sh"))
        .env("PATH", path)
        .env("HOME", sandbox)
        .env("FAKE_RUST_VERSION", version)
        .env("FAKE_CARGO_LOG", &log);
    Ok((cmd, log))
}

#[cfg(unix)]
#[test]
fn check_script_refuses_a_toolchain_other_than_the_pinned_one() -> TestResult {
    let toolchain = repo_toml("rust-toolchain.toml")?;
    let pinned = lookup_str(&toolchain, "toolchain.channel").ok_or("missing channel")?;
    let sandbox = tempfile::TempDir::new()?;
    let (mut cmd, log) = check_script_with_rust(sandbox.path(), "0.0.1")?;

    cmd.assert()
        .failure()
        .stderr(predicates::str::contains(format!(
            "rust-toolchain.toml pins Rust {pinned} but `rustc --version` reports 0.0.1"
        )));
    assert!(!log.exists(), "no check may run on an unpinned toolchain");
    Ok(())
}

#[cfg(unix)]
#[test]
fn check_script_runs_every_blocking_check_on_the_pinned_toolchain() -> TestResult {
    let toolchain = repo_toml("rust-toolchain.toml")?;
    let pinned = lookup_str(&toolchain, "toolchain.channel").ok_or("missing channel")?;
    let sandbox = tempfile::TempDir::new()?;
    let (mut cmd, log) = check_script_with_rust(sandbox.path(), pinned)?;

    cmd.assert().success();
    assert_eq!(
        fs::read_to_string(log)?,
        "fmt --all --check\n\
         clippy --all-targets --all-features --locked -- -D warnings\n\
         test --all-features --locked\n\
         deny check\n"
    );
    Ok(())
}

#[test]
fn consultative_checks_never_gate_pushes_or_pull_requests() -> TestResult {
    let workflow = repo_file(".github/workflows/consultative.yml")?;
    assert!(workflow.contains("\n  schedule:"));
    assert!(workflow.contains("\n  workflow_dispatch:"));
    assert!(!workflow.contains("\n  push:"));
    assert!(!workflow.contains("\n  pull_request"));
    assert!(workflow.contains("cargo mutants"));
    assert!(
        workflow.contains("mutants=\"$(cargo mutants --list)\"")
            && workflow.contains("if [ -z \"$mutants\" ]; then"),
        "an empty mutation selection must fail the job instead of passing silently"
    );
    assert!(workflow.contains("jscpd --config .jscpd.json"));

    let mutants = repo_toml(".cargo/mutants.toml")?;
    let globs = lookup(&mutants, "examine_globs")
        .and_then(toml::Value::as_array)
        .ok_or("missing examine_globs")?;
    assert!(!globs.is_empty(), "mutation testing must stay targeted");
    Ok(())
}

#[test]
fn no_node_or_bun_tooling_is_required() -> TestResult {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for file in ["package.json", "bun.lockb", "bun.lock", "package-lock.json"] {
        assert!(!root.join(file).exists(), "{file} must not exist");
    }
    for workflow in [
        ".github/workflows/ci.yml",
        ".github/workflows/consultative.yml",
    ] {
        let text = repo_file(workflow)?;
        for tool in ["setup-node", "npm ", "npx ", "bun "] {
            assert!(!text.contains(tool), "{workflow} uses {tool}");
        }
    }
    Ok(())
}
