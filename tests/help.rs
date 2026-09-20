//! The help text, held to the document it is specified in.
//!
//! `docs/cli.md` says `cli.rs` must produce a particular block, so this test reads that
//! block out of the page and compares it with what the built binary prints. The helper
//! text is generated from the flag definitions rather than written out twice, which is the
//! property the comparison is there to keep.
//!
//! One normalisation is applied: clap aligns the descriptions in the option list at the
//! column its own layout computes, and the page's block is aligned by hand one space
//! further right. Every other byte, in every other section, is compared exactly.

// An integration test is its own crate, which `clippy.toml`'s `allow-expect-in-tests`
// cannot see: as in `src`'s unit tests, a panic here is the assertion mechanism.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::process::{Command, Stdio};

/// The help block from `docs/cli.md`, the specification this must match.
fn documented_help() -> String {
    let page = include_str!("../docs/cli.md");
    let section = page
        .split_once("## Help text")
        .expect("the page has a Help text section")
        .1;
    let fenced = section
        .split_once("```text")
        .expect("the section has a fenced block")
        .1;
    let block = fenced.split_once("```").expect("the block is closed").0;
    block.trim_matches('\n').to_string()
}

/// Run the binary with a cleared environment and capture stdout.
fn help(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_decide"))
        .args(args)
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .expect("the binary runs");
    assert_eq!(output.status.code(), Some(0), "{args:?} should exit 0");
    assert!(output.stderr.is_empty(), "{args:?} should print to stdout");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Collapse the whitespace inside an option line, so that the two hand-alignments are
/// comparable, and leave every other line exactly as it is.
fn normalise(block: &str) -> Vec<String> {
    block
        .lines()
        .map(|line| {
            let body = line.trim_start();
            if !body.starts_with('-') {
                return line.to_string();
            }
            // The indentation is meaningful, so it is rebuilt rather than collapsed.
            let indent = " ".repeat(line.len().saturating_sub(body.len()));
            let words = body.split_whitespace().collect::<Vec<_>>();
            let (flag, rest) = words.split_first().map_or(("", &[][..]), |(f, r)| (*f, r));
            if rest.is_empty() {
                format!("{indent}{flag}")
            } else {
                format!("{indent}{flag} {}", rest.join(" "))
            }
        })
        .collect()
}

#[test]
fn the_help_is_the_block_the_document_specifies() {
    let documented = documented_help();
    let printed = help(&["--help"]);

    assert_eq!(normalise(&documented), normalise(&printed));
}

#[test]
fn every_flag_the_interface_promises_is_in_the_help() {
    let printed = help(&["--help"]);

    for flag in [
        "--base-url <URL>",
        "--api-key-file <PATH>",
        "--timeout <SECONDS>",
        "--retries <N>",
        "--backoff-ms <MS>",
        "--select <PATH>",
        "--pretty",
        "--verbose",
    ] {
        assert!(printed.contains(flag), "{flag} is missing from --help");
    }
    assert!(printed.contains("TYPESAFE_BASE_URL"), "the env is named");
    assert!(
        printed.contains("TYPESAFE_API_KEY_FILE"),
        "the env is named"
    );
    assert!(
        printed.contains("default: https://api.typesafe.ai"),
        "the default is named"
    );
    assert!(
        printed.contains("2 the invocation is wrong"),
        "the exit codes are in the help"
    );
}

#[test]
fn the_help_says_a_credential_does_not_come_from_a_flag() {
    let printed = help(&["--help"]);

    assert!(
        !printed.contains("--api-key "),
        "there is no --api-key flag"
    );
    assert!(printed.contains("It is not read from a flag"), "{printed}");
}

#[test]
fn a_subcommand_help_carries_the_evaluation_flags() {
    let printed = help(&["noul", "--help"]);

    for flag in [
        "--state",
        "--model",
        "--value",
        "--field",
        "--dry-run",
        "--yes",
        "--no",
    ] {
        assert!(
            printed.contains(flag),
            "{flag} should be in `decide noul --help`, so a caller never goes back"
        );
    }
}

#[test]
fn models_help_has_no_evaluation_flags() {
    let printed = help(&["models", "--help"]);

    for flag in [
        "--state",
        "--state-file",
        "--state-json",
        "--no-state",
        "--model",
        "--dry-run",
    ] {
        assert!(
            !printed.contains(flag),
            "{flag} could not do anything for models, so it is absent rather than ignored"
        );
    }
    for flag in ["--base-url", "--select", "--pretty", "--verbose"] {
        assert!(printed.contains(flag), "{flag} is global and belongs here");
    }
}

#[test]
fn every_subcommand_has_an_example() {
    for subcommand in ["ask", "noul", "choice", "score"] {
        let printed = help(&[subcommand, "--help"]);
        assert!(
            printed.contains("  decide "),
            "`decide {subcommand} --help` should carry a worked example"
        );
    }
}

#[test]
fn the_usage_line_names_the_subcommand_form() {
    assert!(help(&["--help"]).contains("Usage: decide [OPTIONS] <COMMAND>"));
    assert!(help(&["noul", "--help"]).contains("Usage: decide noul"));
    assert!(help(&["models", "--help"]).contains("Usage: decide models"));
}
