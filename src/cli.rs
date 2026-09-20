//! The command line: the clap types and the help text.
//!
//! This is the only module that knows about `argv`. Everything else deals in values, which
//! is what makes the parsing rules testable through [`Cli::try_parse_from`] instead of
//! through a subprocess.
//!
//! Two things here are deliberate and easy to "fix" wrongly:
//!
//! - **`--option` and `--level` are not `required`.** clap's message would be fine; the
//!   point is that `decide`'s own message counts what was given ("none were given") and
//!   names the question, and that the 2..=10 bound on levels is the API's knowledge rather
//!   than clap's.
//! - **Cross-set conflicts are declared on the local flag.** `--select` is global and
//!   `--value`, `--field`, and `--dry-run` are not, and a `conflicts_with` naming a global
//!   id from a local flag is the arrangement clap resolves in every subcommand that has
//!   both.
//!
//! The help text is built from these definitions rather than assembled as a string, so the
//! flag list cannot drift away from the flags that exist. `docs/cli.md` holds the block it
//! must produce; `tests/help.rs` compares the two, with the option list's description
//! column normalised, because clap aligns that column at the width its own layout computes
//! and the page's block was aligned by hand one space further right.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Where the state comes from, and the knobs that shape one request.
///
/// These are accepted by `ask`, `noul`, `choice`, and `score`, and by none of `models`:
/// `models` has no state to send and no body to name a model in.
#[derive(Debug, Args)]
pub struct EvalArgs {
    /// The state itself; JSON if --state-json is given
    #[arg(long, value_name = "TEXT")]
    pub state: Option<String>,

    /// Read the state from this file; - means stdin
    #[arg(long, value_name = "PATH")]
    pub state_file: Option<PathBuf>,

    /// The state is JSON rather than text
    #[arg(long)]
    pub state_json: bool,

    /// Send null as the state, for a self-contained question
    #[arg(long)]
    pub no_state: bool,

    /// The model or alias that handles the call
    #[arg(long, value_name = "NAME")]
    pub model: Option<String>,

    /// Print the request body and make no call
    #[arg(long, conflicts_with_all = ["select", "value", "field"])]
    pub dry_run: bool,

    /// Print the one answer's own value
    #[arg(long, conflicts_with_all = ["select", "field", "dry_run"])]
    pub value: bool,

    /// Print a dotted path from the one answer
    #[arg(long, value_name = "PATH", conflicts_with_all = ["select", "value", "dry_run"])]
    pub field: Option<String>,
}

/// A knob that shapes every request, wherever it appears.
#[derive(Debug, Args)]
pub struct GlobalArgs {
    /// The API endpoint, or the root it lives under
    #[arg(
        long,
        value_name = "URL",
        env = "TYPESAFE_BASE_URL",
        default_value = crate::wire::DEFAULT_BASE_URL,
        hide_env_values = true,
        global = true
    )]
    pub base_url: String,

    /// Read the API key from a file
    #[arg(
        long,
        value_name = "PATH",
        env = "TYPESAFE_API_KEY_FILE",
        hide_env_values = true,
        global = true
    )]
    pub api_key_file: Option<PathBuf>,

    /// Give up on one attempt after this long
    #[arg(
        long,
        value_name = "SECONDS",
        default_value_t = 60,
        value_parser = clap::value_parser!(u64).range(1..),
        global = true
    )]
    pub timeout: u64,

    /// Retry a failed attempt this many times, at most 10
    #[arg(
        long,
        value_name = "N",
        default_value_t = 2,
        value_parser = clap::value_parser!(u32).range(0..=10),
        global = true
    )]
    pub retries: u32,

    /// Delay before the first retry; doubles up to 5000
    #[arg(long, value_name = "MS", default_value_t = 500, global = true)]
    pub backoff_ms: u64,

    /// Print one value from the response instead of the response
    #[arg(long, value_name = "PATH", global = true)]
    pub select: Option<String>,

    /// Indent the JSON output
    #[arg(long, global = true)]
    pub pretty: bool,

    /// Report progress on stderr
    #[arg(long, global = true)]
    pub verbose: bool,
}

/// Ask Jev for a typed decision, and get JSON a shell script can branch on.
#[derive(Debug, Parser)]
#[command(
    name = "decide",
    version,
    about = "Ask Jev for a typed decision, and get JSON a shell script can branch on.",
    after_help = AFTER_HELP,
    arg_required_else_help = true,
    propagate_version = true
)]
pub struct Cli {
    /// The knobs that apply to every subcommand.
    #[command(flatten)]
    pub global: GlobalArgs,

    /// What to do. There is no default subcommand: guessing which of "ask one question"
    /// and "read a file" was meant is a guess with a wrong answer half the time.
    #[command(subcommand)]
    pub command: Command,
}

/// The five things `decide` can do.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Evaluate the request document in a file, or on stdin
    #[command(long_about = ASK_LONG_ABOUT)]
    Ask {
        /// The request document; - means stdin
        #[arg(value_name = "FILE")]
        file: Option<PathBuf>,

        /// Where the state comes from, and the knobs that shape the request.
        #[command(flatten)]
        eval: EvalArgs,
    },

    /// Ask a yes/no question; print the probability that the answer is yes
    #[command(long_about = NOUL_LONG_ABOUT)]
    Noul {
        /// What to ask
        #[arg(value_name = "INSTRUCTION")]
        instruction: String,

        /// The question id the answer comes back under
        #[arg(long, value_name = "NAME", default_value = "answer")]
        id: String,

        /// What a yes means
        #[arg(long, value_name = "DESC")]
        yes: Option<String>,

        /// What a no means
        #[arg(long, value_name = "DESC")]
        no: Option<String>,

        /// Where the state comes from, and the knobs that shape the request.
        #[command(flatten)]
        eval: EvalArgs,
    },

    /// Ask for one option out of a set you define
    #[command(long_about = CHOICE_LONG_ABOUT)]
    Choice {
        /// What to ask
        #[arg(value_name = "INSTRUCTION")]
        instruction: String,

        /// The question id the answer comes back under
        #[arg(long, value_name = "NAME", default_value = "answer")]
        id: String,

        /// An option, as NAME or NAME=DESCRIPTION; repeat for each option
        #[arg(long, value_name = "NAME[=DESC]")]
        option: Vec<String>,

        /// Where the state comes from, and the knobs that shape the request.
        #[command(flatten)]
        eval: EvalArgs,
    },

    /// Ask for a position along levels you define
    #[command(long_about = SCORE_LONG_ABOUT)]
    Score {
        /// What to ask
        #[arg(value_name = "INSTRUCTION")]
        instruction: String,

        /// The question id the answer comes back under
        #[arg(long, value_name = "NAME", default_value = "answer")]
        id: String,

        /// A level, from first to last; give two to ten
        #[arg(long, value_name = "DESC")]
        level: Vec<String>,

        /// Where the state comes from, and the knobs that shape the request.
        #[command(flatten)]
        eval: EvalArgs,
    },

    /// List the models this account can send
    Models,

    /// Store the API token, so that an environment variable is not needed
    #[command(long_about = AUTH_LONG_ABOUT)]
    Auth {
        /// What to do with the store.
        #[command(subcommand)]
        command: AuthCommand,
    },
}

/// What `decide auth` can do with the one secret it keeps.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum AuthCommand {
    /// Read a token from stdin and store it
    #[command(long_about = AUTH_SET_LONG_ABOUT)]
    Set,

    /// Forget the stored token
    #[command(long_about = AUTH_UNSET_LONG_ABOUT)]
    Unset,

    /// Say where the credential comes from, without printing it
    #[command(long_about = AUTH_STATUS_LONG_ABOUT)]
    Status,
}

impl Command {
    /// The evaluation flags, for the subcommands that have them.
    #[must_use]
    pub fn eval(&self) -> Option<&EvalArgs> {
        match self {
            Self::Ask { eval, .. }
            | Self::Noul { eval, .. }
            | Self::Choice { eval, .. }
            | Self::Score { eval, .. } => Some(eval),
            Self::Models | Self::Auth { .. } => None,
        }
    }
}

/// The `after_help` text of the top-level parser, specified verbatim in
/// [`docs/cli.md`](../docs/cli.md#help-text).
///
/// It comes before the flag list on purpose: a caller that gets the flags wrong is usually
/// a caller that has not found an example yet.
const AFTER_HELP: &str = r#"Examples:
  Ask several questions about one document, in a single call:
      decide ask questions.json --state-file ticket.txt

  The same, with the state piped in:
      cat ticket.txt | decide ask questions.json

  Ask one question, and print just its value:
      decide noul "does this message convey urgency?" --state-file ticket.txt --value

  Choose an option, and print the option's name:
      decide choice "which team should handle this?" --state-file ticket.txt \
        --option "billing=payments and invoices" \
        --option "technical=bugs and outages" \
        --id department --value

  Print the request that would be sent, without calling the API:
      decide score "how frustrated is the customer?" --state-file ticket.txt \
        --level calm --level "frustrated but civil" --level "very angry" --dry-run

  Ask about a document that arrived on stdin, with no state in the questions file:
      cat ticket.txt | decide ask questions.json --select answers.department.choice

  Store the API token, so that an environment variable is not needed:
      printf %s "$TOKEN" | decide auth set

The API key is read from TYPESAFE_API_KEY, from a file named by --api-key-file or
TYPESAFE_API_KEY_FILE, or from the store that `decide auth set` writes. It is never read
from a flag: a flag is visible to every process on the machine, and it is kept in the
shell's history.

stdout carries the response and nothing else. Progress and errors go to stderr.
Exit codes: 0 the evaluation completed, 1 it failed, 2 the invocation is wrong."#;

/// `decide ask --help`'s long description.
const ASK_LONG_ABOUT: &str = r"Evaluate every question in one request document, sharing one state.

The document is the API's own request body, so one written for curl works here
unchanged, and one written here can be sent with curl -d @... . It is also the only
form that can carry structured instructions and criteria; the shorthand subcommands
carry exactly the shapes a shell can carry.

  decide ask questions.json --state-file ticket.txt
  cat ticket.txt | decide ask questions.json
  decide ask questions.json --no-state --dry-run";

/// `decide noul --help`'s long description.
const NOUL_LONG_ABOUT: &str = r#"Ask a yes/no question.

The probability that the answer is yes runs from 0 to 1, and a noul has no confidence:
the probability is the answer, so a rule that gates on confidence has to threshold this
number itself, or ask a choice instead.

  decide noul "does this message convey urgency?" --state-file ticket.txt --value
  decide noul "is this urgent?" --no-state --yes urgent --no routine"#;

/// `decide choice --help`'s long description.
const CHOICE_LONG_ABOUT: &str = r#"Ask for one option out of a set you define.

An option is NAME or NAME=DESCRIPTION; the description is sent as that option's
criteria, and a bare NAME sends null, meaning this option needs no extra detail.

  decide choice "which team should handle this?" --state-file ticket.txt \
    --option billing="invoices, refunds" --option technical="bugs, outages" --value"#;

/// `decide auth --help`'s long description.
const AUTH_LONG_ABOUT: &str = r#"Keep the API token, so that an environment variable is not needed.

The token is read from stdin and never from a flag, because argv is readable by every
process on the machine and is kept in shell history. TYPESAFE_API_KEY still wins over
what is stored, so a script or a CI runner can override it for one call.

  printf %s "$TOKEN" | decide auth set
  decide auth status
  decide auth unset"#;

/// `decide auth set --help`'s long description.
const AUTH_SET_LONG_ABOUT: &str = r#"Read a token from stdin and store it for later calls.

  printf %s "$TYPESAFE_API_KEY" | decide auth set
  decide auth set < key.txt

The file is created readable only by its owner. A terminal is refused rather than read,
because a secret typed at a prompt is echoed to the screen."#;

/// `decide auth unset --help`'s long description.
const AUTH_UNSET_LONG_ABOUT: &str = r"Forget the stored token.

  decide auth unset

Nothing else is touched: an environment variable or a file named with --api-key-file
still supplies a credential after this.";

/// `decide auth status --help`'s long description.
const AUTH_STATUS_LONG_ABOUT: &str = r"Say which of the three sources supplies the credential.

  decide auth status

The order is TYPESAFE_API_KEY, then --api-key-file or TYPESAFE_API_KEY_FILE, then the
store. The value is never printed.";

/// `decide score --help`'s long description.
const SCORE_LONG_ABOUT: &str = r#"Ask for a position along ordered levels you define.

The order of --level is the order of the levels, from first to last, and there is no
other way to say which end is which. Two to ten levels are required.

  decide score "how frustrated is the customer?" --state-file ticket.txt \
    --level calm --level "frustrated but civil" --level "very angry" --value"#;

#[cfg(test)]
mod tests {
    use std::path::Path;

    use clap::error::ErrorKind;

    use super::*;

    /// Parse a `decide` invocation, or return clap's refusal.
    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(args)
    }

    /// Parse an invocation that the tests expect to be accepted.
    fn accepted(args: &[&str]) -> Cli {
        parse(args).expect("the invocation is valid")
    }

    /// The evaluation flags of a subcommand that has them.
    fn eval_args(cli: &Cli) -> &EvalArgs {
        cli.command.eval().expect("this subcommand has them")
    }

    #[test]
    fn a_bare_invocation_prints_the_help() {
        let error = parse(&["decide"]).expect_err("there is no default subcommand");

        assert_eq!(
            error.kind(),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
        assert!(
            error.use_stderr(),
            "the help goes to stderr, and the exit is 2"
        );
        assert!(error.to_string().contains("Usage: decide"), "{error}");
    }

    #[test]
    fn flags_without_a_subcommand_are_a_usage_error() {
        let error = parse(&["decide", "--pretty"]).expect_err("there is no default subcommand");

        assert_eq!(error.kind(), ErrorKind::MissingSubcommand);
        assert!(error.use_stderr());
    }

    #[test]
    fn an_unknown_subcommand_is_a_usage_error() {
        let error = parse(&["decide", "sing"]).expect_err("there is no sing");

        assert_eq!(error.kind(), ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn the_extraction_flags_conflict() {
        let conflicts = [
            ["--value", "--field=confidence"],
            ["--select=model", "--value"],
            ["--select=model", "--field=confidence"],
            ["--dry-run", "--select=model"],
            ["--dry-run", "--value"],
            ["--dry-run", "--field=confidence"],
        ];

        for pair in conflicts {
            let mut args = vec!["decide", "noul", "is it so?", "--no-state"];
            args.extend_from_slice(&pair);
            let error = parse(&args).expect_err("these ask for different things to be printed");
            assert_eq!(
                error.kind(),
                ErrorKind::ArgumentConflict,
                "{pair:?} should conflict, but clap said: {error}"
            );
        }
    }

    #[test]
    fn the_api_key_flag_does_not_exist() {
        let error = parse(&["decide", "models", "--api-key", "sekrit"])
            .expect_err("a credential in argv is readable by every process");

        assert_eq!(error.kind(), ErrorKind::UnknownArgument);
    }

    #[test]
    fn models_does_not_accept_an_evaluation_flag() {
        for flag in [
            "--state",
            "--state-file",
            "--no-state",
            "--model",
            "--dry-run",
            "--value",
        ] {
            let error = parse(&["decide", "models", flag, "x"])
                .or_else(|_| parse(&["decide", "models", flag]))
                .expect_err("models has no state and no body");
            assert_eq!(
                error.kind(),
                ErrorKind::UnknownArgument,
                "{flag} should be absent from models, not ignored"
            );
        }
    }

    #[test]
    fn option_and_level_are_not_required_so_decide_does_the_counting() {
        let cli = accepted(&["decide", "choice", "which?", "--no-state"]);
        let Command::Choice { option, .. } = &cli.command else {
            panic!("that is the choice subcommand");
        };
        assert!(option.is_empty(), "clap does not own the count rule");

        let cli = accepted(&["decide", "score", "how much?", "--no-state"]);
        let Command::Score { level, .. } = &cli.command else {
            panic!("that is the score subcommand");
        };
        assert!(level.is_empty(), "clap does not own the count rule");
    }

    #[test]
    fn options_keep_the_order_they_were_given_in() {
        let cli = accepted(&[
            "decide",
            "choice",
            "which?",
            "--no-state",
            "--option",
            "billing",
            "--option",
            "technical=bugs",
            "--option",
            "sales",
        ]);

        let Command::Choice { option, .. } = &cli.command else {
            panic!("that is the choice subcommand");
        };
        assert_eq!(
            option,
            &vec![
                "billing".to_string(),
                "technical=bugs".to_string(),
                "sales".to_string()
            ]
        );
    }

    #[test]
    fn the_question_id_defaults_to_answer() {
        let cli = accepted(&["decide", "noul", "is it so?", "--no-state"]);
        let Command::Noul { id, .. } = &cli.command else {
            panic!("that is the noul subcommand");
        };
        assert_eq!(id, "answer");

        let cli = accepted(&[
            "decide",
            "noul",
            "is it so?",
            "--no-state",
            "--id",
            "urgency",
        ]);
        let Command::Noul { id, .. } = &cli.command else {
            panic!("that is the noul subcommand");
        };
        assert_eq!(id, "urgency");
    }

    #[test]
    fn the_yes_and_no_descriptions_are_optional() {
        let cli = accepted(&[
            "decide",
            "noul",
            "is it so?",
            "--no-state",
            "--yes",
            "urgent",
        ]);

        let Command::Noul { yes, no, .. } = &cli.command else {
            panic!("that is the noul subcommand");
        };
        assert_eq!(yes.as_deref(), Some("urgent"));
        assert_eq!(no, &None);
    }

    #[test]
    fn the_state_flags_parse() {
        let cli = accepted(&[
            "decide",
            "noul",
            "is it so?",
            "--state",
            "text",
            "--state-json",
            "--model",
            "m",
        ]);
        let eval = eval_args(&cli);
        assert_eq!(eval.state.as_deref(), Some("text"));
        assert!(eval.state_json);
        assert_eq!(eval.model.as_deref(), Some("m"));

        let cli = accepted(&["decide", "noul", "is it so?", "--no-state"]);
        assert!(eval_args(&cli).no_state);

        let cli = accepted(&["decide", "noul", "is it so?", "--state-file", "-"]);
        assert_eq!(eval_args(&cli).state_file.as_deref(), Some(Path::new("-")));
    }

    #[test]
    fn the_ask_file_is_optional_and_dash_is_an_ordinary_value() {
        let cli = accepted(&["decide", "ask", "--no-state"]);
        let Command::Ask { file, .. } = &cli.command else {
            panic!("that is the ask subcommand");
        };
        assert_eq!(file, &None);

        let cli = accepted(&["decide", "ask", "questions.json", "--no-state"]);
        let Command::Ask { file, .. } = &cli.command else {
            panic!("that is the ask subcommand");
        };
        assert_eq!(file.as_deref(), Some(Path::new("questions.json")));
    }

    #[test]
    fn auth_has_three_actions() {
        for (argument, expected) in [
            ("set", AuthCommand::Set),
            ("unset", AuthCommand::Unset),
            ("status", AuthCommand::Status),
        ] {
            let cli = accepted(&["decide", "auth", argument]);
            let Command::Auth { command } = &cli.command else {
                panic!("that is the auth subcommand");
            };
            assert_eq!(command, &expected, "decide auth {argument}");
        }
    }

    #[test]
    fn auth_without_an_action_prints_its_own_help() {
        let error = parse(&["decide", "auth"]).expect_err("there is nothing to do");

        assert_eq!(
            error.kind(),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
        assert!(
            error.use_stderr(),
            "exit 2, and the help is a diagnostic here"
        );
        assert!(error.to_string().contains("Usage: decide auth"), "{error}");
        assert!(error.to_string().contains("status"), "{error}");
    }

    #[test]
    fn an_unrecognised_auth_action_is_a_usage_error() {
        let error = parse(&["decide", "auth", "forget"]).expect_err("there is no forget");

        assert_eq!(error.kind(), ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn auth_takes_no_evaluation_flags() {
        for flag in ["--state", "--model", "--dry-run", "--value"] {
            let error = parse(&["decide", "auth", "set", flag, "x"])
                .or_else(|_| parse(&["decide", "auth", "set", flag]))
                .expect_err("auth has no request to shape");
            assert_eq!(
                error.kind(),
                ErrorKind::UnknownArgument,
                "{flag} should be absent from auth, not ignored"
            );
        }
    }

    #[test]
    fn auth_takes_no_credential_flag_either() {
        let error = parse(&["decide", "auth", "set", "--api-key", "sekrit"])
            .expect_err("the token is read from stdin");

        assert_eq!(error.kind(), ErrorKind::UnknownArgument);
    }

    #[test]
    fn global_flags_are_accepted_before_or_after_the_subcommand() {
        let before = accepted(&[
            "decide",
            "--select",
            "model",
            "noul",
            "is it so?",
            "--no-state",
        ]);
        assert_eq!(before.global.select.as_deref(), Some("model"));

        let after = accepted(&[
            "decide",
            "noul",
            "is it so?",
            "--no-state",
            "--select",
            "model",
        ]);
        assert_eq!(after.global.select.as_deref(), Some("model"));
    }

    #[test]
    fn the_global_defaults_are_the_documented_ones() {
        let cli = accepted(&["decide", "models"]);

        assert_eq!(cli.global.base_url, crate::wire::DEFAULT_BASE_URL);
        assert_eq!(cli.global.base_url, "https://api.typesafe.ai/v1/systemone");
        assert_eq!(cli.global.timeout, 60);
        assert_eq!(cli.global.retries, 2);
        assert_eq!(cli.global.backoff_ms, 500);
        assert!(!cli.global.pretty);
        assert!(!cli.global.verbose);
        assert_eq!(cli.global.select, None);
    }

    #[test]
    fn a_timeout_of_zero_is_refused_rather_than_meaning_forever() {
        let error =
            parse(&["decide", "models", "--timeout", "0"]).expect_err("zero is not forever");

        assert_eq!(error.kind(), ErrorKind::ValueValidation);
    }

    #[test]
    fn more_than_ten_retries_is_refused() {
        let error =
            parse(&["decide", "models", "--retries", "11"]).expect_err("eleven is too many");

        assert_eq!(error.kind(), ErrorKind::ValueValidation);
        assert!(parse(&["decide", "models", "--retries", "0"]).is_ok());
        assert!(parse(&["decide", "models", "--retries", "10"]).is_ok());
    }

    #[test]
    fn models_has_no_evaluation_flags_at_all() {
        let cli = accepted(&["decide", "models"]);

        assert!(cli.command.eval().is_none());
    }

    #[test]
    fn auth_asks_no_question() {
        let cli = accepted(&["decide", "auth", "status"]);

        assert!(cli.command.eval().is_none());
    }

    #[test]
    fn an_instruction_is_required_by_the_shorthand_subcommands() {
        for subcommand in ["noul", "choice", "score"] {
            let error = parse(&["decide", subcommand]).expect_err("there is nothing to ask");
            assert_eq!(
                error.kind(),
                ErrorKind::MissingRequiredArgument,
                "{subcommand}"
            );
        }
    }
}
