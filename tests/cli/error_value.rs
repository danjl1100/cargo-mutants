// Copyright 2023 Martin Pool

//! Tests for error value mutations, from `--error-value` etc.

use std::env;

use indoc::indoc;
use predicates::prelude::*;

use super::{copy_of_testdata, run};

#[test]
fn error_value_catches_untested_ok_case() {
    // By default this tree should fail because it's configured to
    // generate an error value, and the tests forgot to check that
    // the code under test does return Ok.
    let tmp_src_dir = copy_of_testdata("error_value");
    run()
        .arg("mutants")
        .args(["-v", "-V", "--no-times", "--no-shuffle"])
        .arg("-d")
        .arg(tmp_src_dir.path())
        .assert()
        .code(2)
        .stderr("")
        .stdout(predicate::function(|stdout| {
            insta::assert_snapshot!(stdout);
            true
        }));
}

#[test]
fn no_config_option_disables_config_file_so_error_value_is_not_generated() {
    // In this case, the config file is not loaded. Error values are not
    // generated by default (because we don't know what a good value for
    // this tree would be), so no mutants are caught.
    let tmp_src_dir = copy_of_testdata("error_value");
    run()
        .arg("mutants")
        .args(["-v", "-V", "--no-times", "--no-shuffle", "--no-config"])
        .arg("-d")
        .arg(tmp_src_dir.path())
        .assert()
        .code(0)
        .stderr("")
        .stdout(predicate::function(|stdout| {
            insta::assert_snapshot!(stdout);
            true
        }));
}

#[test]
fn list_mutants_with_error_value_from_command_line_list() {
    // This is not a good error mutant for this tree, which uses
    // anyhow, but it's a good test of the command line option.
    let tmp_src_dir = copy_of_testdata("error_value");
    run()
        .arg("mutants")
        .args([
            "--no-times",
            "--no-shuffle",
            "--no-config",
            "--list",
            "--error=::eyre::eyre!(\"mutant\")",
        ])
        .arg("-d")
        .arg(tmp_src_dir.path())
        .assert()
        .code(0)
        .stderr("")
        .stdout(predicate::function(|stdout| {
            insta::assert_snapshot!(stdout);
            true
        }));
}

#[test]
fn warn_if_error_value_starts_with_err() {
    // Users might misunderstand what should be passed to --error,
    // so give a warning.
    let tmp_src_dir = copy_of_testdata("error_value");
    run()
        .arg("mutants")
        .args([
            "--no-times",
            "--no-shuffle",
            "--no-config",
            "--list",
            "--error=Err(anyhow!(\"mutant\"))",
        ])
        .arg("-d")
        .arg(tmp_src_dir.path())
        .assert()
        .code(0)
        .stderr(predicate::str::contains(
            "error_value option gives the value of the error, and probably should not start with Err(: got Err(anyhow!(\"mutant\"))"
        ))
        .stdout(indoc! { "\
            src/lib.rs:4:5: replace even_is_ok -> Result<u32, &\'static str> with Ok(0)
            src/lib.rs:4:5: replace even_is_ok -> Result<u32, &\'static str> with Ok(1)
            src/lib.rs:4:5: replace even_is_ok -> Result<u32, &\'static str> with Err(Err(anyhow!(\"mutant\")))
            src/lib.rs:4:14: replace == with != in even_is_ok
        " });
}

#[test]
fn fail_when_error_value_does_not_parse() {
    let tmp_src_dir = copy_of_testdata("error_value");
    run()
        .arg("mutants")
        .args([
            "--no-times",
            "--no-shuffle",
            "--no-config",
            "--list",
            "--error=shouldn't work",
        ])
        .arg("-d")
        .arg(tmp_src_dir.path())
        .assert()
        .code(1)
        .stderr(predicate::str::contains(indoc! { "
            Error: Failed to parse error value \"shouldn\'t work\"

            Caused by:
                unexpected token
        "}))
        .stdout(predicate::str::is_empty());
}
