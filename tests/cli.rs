//! Command-line contract of the `skill-curator` binary: help, version, exit
//! codes, and error diagnostics.

use std::error::Error;
use std::path::Path;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn Error>>;

const SUBCOMMANDS: [&str; 5] = ["status", "check", "update", "materialize", "install"];

/// Exit code for usage errors reported by the argument parser.
const USAGE_ERROR: i32 = 2;
/// Exit code for failures after the arguments were accepted.
const FAILURE: i32 = 1;

/// Builds a `skill-curator` invocation isolated from the user's real home,
/// configuration, and working directory.
fn skill_curator(sandbox: &Path) -> Command {
    let mut cmd = cargo_bin_cmd!("skill-curator");
    cmd.current_dir(sandbox)
        .env("HOME", sandbox)
        .env("XDG_CONFIG_HOME", sandbox.join(".config"))
        .env("XDG_STATE_HOME", sandbox.join(".local/state"))
        .env("XDG_CACHE_HOME", sandbox.join(".cache"))
        .env_remove("RUST_BACKTRACE");
    cmd
}

#[test]
fn help_lists_every_v1_subcommand_and_global_flag() -> TestResult {
    let sandbox = TempDir::new()?;
    let mut assert = skill_curator(sandbox.path())
        .arg("--help")
        .assert()
        .success();
    for name in SUBCOMMANDS {
        assert = assert.stdout(predicate::str::contains(name));
    }
    assert
        .stdout(predicate::str::contains("--collection <PATH>"))
        .stdout(predicate::str::contains("--machine-config <PATH>"))
        .stdout(predicate::str::contains("--version"))
        .stderr(predicate::str::is_empty());
    Ok(())
}

#[test]
fn help_describes_subcommands_with_glossary_terms() -> TestResult {
    let sandbox = TempDir::new()?;
    skill_curator(sandbox.path())
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Collection"))
        .stdout(predicate::str::contains("Destination"))
        .stdout(predicate::str::contains("Revision"));
    Ok(())
}

#[test]
fn version_flag_prints_package_version() -> TestResult {
    let sandbox = TempDir::new()?;
    let expected = format!("skill-curator {}\n", env!("CARGO_PKG_VERSION"));
    for flag in ["--version", "-V"] {
        skill_curator(sandbox.path())
            .arg(flag)
            .assert()
            .success()
            .stdout(expected.clone())
            .stderr(predicate::str::is_empty());
    }
    Ok(())
}

#[test]
fn missing_subcommand_is_a_usage_error_showing_help() -> TestResult {
    let sandbox = TempDir::new()?;
    skill_curator(sandbox.path())
        .assert()
        .code(USAGE_ERROR)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("Usage: skill-curator"));
    Ok(())
}

#[test]
fn unknown_subcommand_is_a_usage_error() -> TestResult {
    let sandbox = TempDir::new()?;
    skill_curator(sandbox.path())
        .arg("frobnicate")
        .assert()
        .code(USAGE_ERROR)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "unrecognized subcommand 'frobnicate'",
        ));
    Ok(())
}

#[test]
fn unknown_flag_is_a_usage_error() -> TestResult {
    let sandbox = TempDir::new()?;
    skill_curator(sandbox.path())
        .args(["status", "--no-such-flag"])
        .assert()
        .code(USAGE_ERROR)
        .stderr(predicate::str::contains(
            "unexpected argument '--no-such-flag'",
        ));
    Ok(())
}

#[test]
fn global_flag_requires_a_value() -> TestResult {
    let sandbox = TempDir::new()?;
    skill_curator(sandbox.path())
        .args(["status", "--collection"])
        .assert()
        .code(USAGE_ERROR)
        .stderr(predicate::str::contains("--collection <PATH>"));
    Ok(())
}

#[test]
fn every_subcommand_reports_not_implemented_yet() -> TestResult {
    let sandbox = TempDir::new()?;
    for name in SUBCOMMANDS {
        skill_curator(sandbox.path())
            .arg(name)
            .assert()
            .code(FAILURE)
            .stdout(predicate::str::is_empty())
            .stderr(format!(
                "skill-curator: error: the `{name}` command is not implemented yet\n"
            ));
    }
    Ok(())
}

#[test]
fn global_flags_are_accepted_before_and_after_the_subcommand() -> TestResult {
    let sandbox = TempDir::new()?;
    let collection = sandbox.path().join("collection");
    let machine_config = sandbox.path().join("machine.toml");
    let collection = collection.to_str().ok_or("non-UTF-8 temporary path")?;
    let machine_config = machine_config.to_str().ok_or("non-UTF-8 temporary path")?;

    let invocations = [
        vec![
            "--collection",
            collection,
            "--machine-config",
            machine_config,
            "status",
        ],
        vec![
            "status",
            "--collection",
            collection,
            "--machine-config",
            machine_config,
        ],
    ];
    for args in invocations {
        skill_curator(sandbox.path())
            .args(&args)
            .assert()
            .code(FAILURE)
            .stderr(predicate::str::contains(
                "`status` command is not implemented yet",
            ));
    }
    Ok(())
}

#[test]
fn subcommand_help_succeeds() -> TestResult {
    let sandbox = TempDir::new()?;
    for name in SUBCOMMANDS {
        skill_curator(sandbox.path())
            .args([name, "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains(format!(
                "Usage: skill-curator {name}"
            )));
    }
    Ok(())
}
