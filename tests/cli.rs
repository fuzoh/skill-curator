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

/// Subcommands that have no behaviour yet.
const NOT_IMPLEMENTED: [&str; 4] = ["status", "check", "update", "install"];

/// Exit code for usage errors reported by the argument parser.
const USAGE_ERROR: i32 = 2;
/// Exit code for failures after the arguments were accepted.
const FAILURE: i32 = 1;

/// Builds a `skill-curator` invocation isolated from the user's real home,
/// configuration, working directory, and environment. The inherited
/// environment is cleared so that variables from the caller's shell (for
/// example `CLICOLOR_FORCE`, which makes clap emit ANSI escapes) cannot change
/// the output under test. Only `PATH` is passed through, for locating external
/// programs.
fn skill_curator(sandbox: &Path) -> Command {
    let mut cmd = cargo_bin_cmd!("skill-curator");
    cmd.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.current_dir(sandbox)
        .env("HOME", sandbox)
        .env("XDG_CONFIG_HOME", sandbox.join(".config"))
        .env("XDG_STATE_HOME", sandbox.join(".local/state"))
        .env("XDG_CACHE_HOME", sandbox.join(".cache"));
    cmd
}

/// Set in the re-run started by
/// `cli_contract_holds_when_the_caller_environment_is_hostile`, so the re-run
/// does not start another one.
const HOSTILE_RERUN_MARKER: &str = "SKILL_CURATOR_TEST_HOSTILE_RERUN";

/// Runs every test of this binary again with inherited environment variables
/// that would change the output under test, such as forced colour. The tests
/// must still pass, which proves `skill_curator` does not leak the caller's
/// environment into the binary under test.
#[test]
fn cli_contract_holds_when_the_caller_environment_is_hostile() -> TestResult {
    if std::env::var_os(HOSTILE_RERUN_MARKER).is_some() {
        return Ok(());
    }
    let output = std::process::Command::new(std::env::current_exe()?)
        .env(HOSTILE_RERUN_MARKER, "1")
        .env("CLICOLOR_FORCE", "1")
        .env("CLICOLOR", "1")
        .env("RUST_BACKTRACE", "1")
        .env("XDG_CONFIG_HOME", "/nonexistent/skill-curator-test-config")
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "CLI tests fail when the caller environment is hostile:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("test help_lists_every_v1_subcommand_and_global_flag ... ok"),
        "the hostile-environment re-run did not run the CLI tests:\n{stdout}"
    );
    Ok(())
}

/// Names of the entries in the `Commands:` section of `--help` output. A
/// command counts as listed only through its own entry, never through words in
/// another entry's description or a wrapped description line.
fn listed_commands(help: &str) -> Vec<&str> {
    help.lines()
        .skip_while(|line| line.trim_end() != "Commands:")
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .filter_map(|line| line.strip_prefix("  "))
        .filter(|entry| !entry.starts_with(char::is_whitespace))
        .filter_map(|entry| entry.split_whitespace().next())
        .collect()
}

#[test]
fn listed_commands_ignores_names_mentioned_only_in_descriptions() {
    let help = "Usage: tool <COMMAND>\n\
                \n\
                Commands:\n  \
                status  Report known upstream updates\n  \
                check   Fetch Sources\n          \
                update wrapped description line\n\
                \n\
                Options:\n  \
                -h, --help  Print help\n";
    assert_eq!(listed_commands(help), ["status", "check"]);
}

#[test]
fn help_lists_every_v1_subcommand_and_global_flag() -> TestResult {
    let sandbox = TempDir::new()?;
    let assert = skill_curator(sandbox.path())
        .arg("--help")
        .assert()
        .success();
    let help = String::from_utf8(assert.get_output().stdout.clone())?;
    let listed = listed_commands(&help);
    for name in SUBCOMMANDS {
        assert!(
            listed.contains(&name),
            "`{name}` is not listed under Commands: in --help output:\n{help}"
        );
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
fn unimplemented_subcommands_report_not_implemented_yet() -> TestResult {
    let sandbox = TempDir::new()?;
    for name in NOT_IMPLEMENTED {
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
fn materialize_outside_a_collection_fails_without_writing() -> TestResult {
    let sandbox = TempDir::new()?;
    skill_curator(sandbox.path())
        .arg("materialize")
        .assert()
        .code(FAILURE)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::starts_with("skill-curator: error: "))
        .stderr(predicate::str::contains("manifest.toml"));
    assert_eq!(std::fs::read_dir(sandbox.path())?.count(), 0);
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
