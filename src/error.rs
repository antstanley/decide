//! The one error type, and the exit code its kind implies.
//!
//! Every variant is on one side of the split in
//! [D11](../docs/design.md)'s split:
//! the invocation was wrong (exit `2`) or the evaluation was attempted and did not
//! complete (exit `1`). There is no third kind, because a third kind is a bug.
//!
//! The `Display` of each variant names the thing that is wrong — the flag, the question id,
//! the key, the path, or the status — because the caller that reads it may never be able to
//! read this source.

use std::path::PathBuf;
use thiserror::Error;

/// Everything `decide` can refuse or fail with.
#[derive(Debug, Error)]
pub enum DecideError {
    // ---- the invocation is wrong: exit 2 ----------------------------------------
    /// No credential was found in the environment, in a named file, or in the store.
    #[error(
        "no API key: set TYPESAFE_API_KEY, name a file with --api-key-file or \
         TYPESAFE_API_KEY_FILE, or store one with `decide auth set`"
    )]
    MissingApiKey,

    /// There is no directory to keep a stored credential in.
    #[error(
        "cannot tell where to keep the credential: neither XDG_CONFIG_HOME nor HOME is set, \
         so set TYPESAFE_API_KEY or name a file with --api-key-file"
    )]
    NoCredentialStore,

    /// The store could not be read.
    #[error("cannot read the stored credential at \"{}\": {reason}", path.display())]
    CredentialUnreadable {
        /// The path that was consulted.
        path: PathBuf,
        /// The OS error, verbatim.
        reason: String,
    },

    /// The store could not be written.
    #[error("cannot store the credential at \"{}\": {reason}", path.display())]
    CredentialUnwritable {
        /// The path that was written.
        path: PathBuf,
        /// The OS error, verbatim.
        reason: String,
    },

    /// `decide auth set` was given a terminal rather than a pipe.
    #[error(
        "read the token from stdin, not from a terminal: pipe it in, as in \
         `printf %s \"$TOKEN\" | decide auth set`"
    )]
    TokenFromTerminal,

    /// `decide auth set` read nothing.
    #[error("the token read from stdin is empty; nothing was stored")]
    EmptyToken,

    /// The file named as the credential could not be read.
    #[error("cannot read the API key file \"{}\": {reason}", path.display())]
    ApiKeyFileUnreadable {
        /// The path that was named.
        path: PathBuf,
        /// The OS error, verbatim.
        reason: String,
    },

    /// The file named as the credential was empty.
    #[error("the API key file \"{}\" is empty", path.display())]
    ApiKeyFileEmpty {
        /// The path that was named.
        path: PathBuf,
    },

    /// The base URL had no scheme, so it would have become a relative URL.
    #[error("--base-url \"{url}\" has no scheme; a base URL must begin with http:// or https://")]
    BaseUrlWithoutScheme {
        /// The URL as it was given.
        url: String,
    },

    /// Two state sources were named, and there is no defensible winner.
    #[error("the state came from two places: {first} and {second}; name exactly one")]
    TwoStateSources {
        /// The first source, by the name a caller would use.
        first: &'static str,
        /// The second source, by the name a caller would use.
        second: &'static str,
    },

    /// No state source was named and there was nothing on stdin to read.
    #[error("no state: name --state, --state-file, or --no-state, or pipe one in")]
    NoStateSource,

    /// A text state had no non-whitespace character, which is what an empty pipeline
    /// looks like.
    #[error("the state is whitespace only; pass --no-state to ask about no state at all")]
    EmptyState,

    /// `--state-json` was given but the state is not valid JSON.
    #[error("--state-json: the state is not valid JSON: {reason}")]
    StateNotJson {
        /// The parser's own complaint.
        reason: String,
    },

    /// The state was a value the API does not accept.
    #[error("the state is {kind}; the API accepts a string, an object, or an array")]
    StateKindUnsupported {
        /// `a number`, `a boolean`, and the like.
        kind: &'static str,
    },

    /// `--state-json` and `--no-state` disagree about whether there is a state.
    #[error("--state-json and --no-state contradict each other: --no-state sends null")]
    StateJsonWithNoState,

    /// `--state-json` was given, but the request document carries the state itself.
    #[error(
        "--state-json was given, but the request document already carries a \"state\", \
         which is JSON by construction"
    )]
    StateJsonWithDocumentState,

    /// `--state-json` was given with nothing for it to describe.
    #[error("--state-json needs a state to describe: give --state, --state-file, or pipe one in")]
    StateJsonWithoutSource,

    /// `ask` was given no document and stdin is a terminal.
    #[error("ask needs a request document: name a file, or pipe one in and pass -")]
    NoDocument,

    /// A file named on the command line could not be read.
    #[error("cannot read \"{}\": {reason}", path.display())]
    UnreadableFile {
        /// The path that was named.
        path: PathBuf,
        /// The OS error, verbatim.
        reason: String,
    },

    /// The request document is not valid JSON.
    #[error("the request document is not valid JSON: {reason}")]
    DocumentNotJson {
        /// The parser's own complaint.
        reason: String,
    },

    /// The request document is JSON, but not an object.
    #[error("the request document is not a JSON object")]
    DocumentNotObject,

    /// Something in the request document is wrong: a key, a shape, or a limit.
    ///
    /// The location names where — `the request document`, or `question "department"` — and
    /// the problem names what, so that a document with a dozen questions still points at
    /// the one that is wrong.
    #[error("{location}: {problem}")]
    Invalid {
        /// Where in the document the problem is.
        location: String,
        /// What is wrong there.
        problem: String,
    },

    /// The same option name was given twice.
    #[error(
        "the option \"{name}\" was given twice; a choice's options are keys, and a key \
         cannot be repeated"
    )]
    DuplicateOption {
        /// The repeated name.
        name: String,
    },

    /// A `--select`, `--value`, or `--field` path did not resolve.
    #[error("{flag} \"{path}\" does not resolve: {problem}")]
    PathNotFound {
        /// The flag the path came from, as the caller spelled it.
        flag: &'static str,
        /// The path, whole, as it was given.
        path: String,
        /// The segment that failed and what was available there.
        problem: String,
    },

    /// `--value` or `--field` was given for a request that did not ask exactly one
    /// question.
    #[error(
        "{flag} needs a request that asked exactly one question (this one asked {count}); \
         use --select to name one answer"
    )]
    OneAnswerRequired {
        /// The flag, as the caller spelled it.
        flag: &'static str,
        /// How many questions the request actually asked.
        count: usize,
    },

    // ---- the evaluation was attempted and did not complete: exit 1 ---------------
    /// The connection failed, and the retries were exhausted.
    #[error("the request failed after {attempts} {}: {reason}", attempt_word(*attempts))]
    Transport {
        /// The transport's own complaint.
        reason: String,
        /// How many attempts were made, including the first.
        attempts: u32,
    },

    /// The attempt took longer than `--timeout` allowed.
    #[error("the request to {url} timed out after {seconds} s")]
    Timeout {
        /// The URL that did not answer in time.
        url: String,
        /// The per-attempt timeout, in seconds.
        seconds: u64,
    },

    /// The API answered with a status that is not a success.
    #[error("the API answered {status} {reason}: {body}")]
    Status {
        /// The status code.
        status: u16,
        /// The reason phrase, or a stand-in when the server sent none.
        reason: String,
        /// The error body, verbatim, capped in length.
        body: String,
    },

    /// The response body was not JSON.
    #[error("the response is not JSON: {reason}")]
    ResponseNotJson {
        /// The parser's own complaint.
        reason: String,
    },

    /// The response parsed, but disagrees with the request.
    #[error("the response broke the answer contract: {problem}")]
    ResponseContract {
        /// What disagreed.
        problem: String,
    },

    /// The answer could not be written to stdout, so the run did not complete.
    #[error("could not write the output: {reason}")]
    Output {
        /// The OS error, verbatim.
        reason: String,
    },
}

impl DecideError {
    /// The process exit code this error implies: `2` for a wrong invocation, `1` for a
    /// failed evaluation.
    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::MissingApiKey
            | Self::NoCredentialStore
            | Self::CredentialUnreadable { .. }
            | Self::CredentialUnwritable { .. }
            | Self::TokenFromTerminal
            | Self::EmptyToken
            | Self::ApiKeyFileUnreadable { .. }
            | Self::ApiKeyFileEmpty { .. }
            | Self::BaseUrlWithoutScheme { .. }
            | Self::TwoStateSources { .. }
            | Self::NoStateSource
            | Self::EmptyState
            | Self::StateNotJson { .. }
            | Self::StateKindUnsupported { .. }
            | Self::StateJsonWithNoState
            | Self::StateJsonWithDocumentState
            | Self::StateJsonWithoutSource
            | Self::NoDocument
            | Self::UnreadableFile { .. }
            | Self::DocumentNotJson { .. }
            | Self::DocumentNotObject
            | Self::Invalid { .. }
            | Self::DuplicateOption { .. }
            | Self::PathNotFound { .. }
            | Self::OneAnswerRequired { .. } => 2,

            Self::Transport { .. }
            | Self::Timeout { .. }
            | Self::Status { .. }
            | Self::ResponseNotJson { .. }
            | Self::ResponseContract { .. }
            | Self::Output { .. } => 1,
        }
    }

    /// Whether this error is the caller's invocation being wrong rather than a failed
    /// evaluation.
    #[must_use]
    pub const fn is_usage(&self) -> bool {
        self.exit_code() == 2
    }
}

/// `attempt` or `attempts`, so that `after 1 attempt` reads.
const fn attempt_word(attempts: u32) -> &'static str {
    if attempts == 1 { "attempt" } else { "attempts" }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn a_usage_error_and_an_evaluation_failure_have_different_exit_codes() {
        assert_eq!(DecideError::NoStateSource.exit_code(), 2);
        assert!(DecideError::NoStateSource.is_usage());

        let failed = DecideError::Status {
            status: 500,
            reason: "Internal Server Error".to_string(),
            body: "later".to_string(),
        };
        assert_eq!(failed.exit_code(), 1);
        assert!(!failed.is_usage());
    }

    #[test]
    fn every_usage_error_is_exit_two() {
        let usage = [
            DecideError::MissingApiKey,
            DecideError::ApiKeyFileUnreadable {
                path: PathBuf::from("key"),
                reason: "no such file".to_string(),
            },
            DecideError::ApiKeyFileEmpty {
                path: PathBuf::from("key"),
            },
            DecideError::BaseUrlWithoutScheme {
                url: "api.typesafe.ai".to_string(),
            },
            DecideError::TwoStateSources {
                first: "--state",
                second: "stdin",
            },
            DecideError::NoStateSource,
            DecideError::EmptyState,
            DecideError::StateNotJson {
                reason: "expected value".to_string(),
            },
            DecideError::StateKindUnsupported { kind: "a number" },
            DecideError::StateJsonWithNoState,
            DecideError::StateJsonWithDocumentState,
            DecideError::StateJsonWithoutSource,
            DecideError::NoDocument,
            DecideError::UnreadableFile {
                path: PathBuf::from("doc"),
                reason: "no such file".to_string(),
            },
            DecideError::DocumentNotJson {
                reason: "expected value".to_string(),
            },
            DecideError::DocumentNotObject,
            DecideError::Invalid {
                location: "the request document".to_string(),
                problem: "unknown key".to_string(),
            },
            DecideError::DuplicateOption {
                name: "billing".to_string(),
            },
            DecideError::PathNotFound {
                flag: "--select",
                path: "answers.x".to_string(),
                problem: "no key \"x\"".to_string(),
            },
            DecideError::OneAnswerRequired {
                flag: "--value",
                count: 3,
            },
        ];
        for error in &usage {
            assert_eq!(error.exit_code(), 2, "{error} should be a usage error");
            assert!(error.is_usage(), "{error}");
        }
    }

    #[test]
    fn every_evaluation_failure_is_exit_one() {
        let failures = [
            DecideError::Transport {
                reason: "connection refused".to_string(),
                attempts: 3,
            },
            DecideError::Timeout {
                url: "http://localhost/v1/systemone".to_string(),
                seconds: 1,
            },
            DecideError::Status {
                status: 422,
                reason: "Unprocessable Entity".to_string(),
                body: "no".to_string(),
            },
            DecideError::ResponseNotJson {
                reason: "expected value".to_string(),
            },
            DecideError::ResponseContract {
                problem: "no answer for question \"a\"".to_string(),
            },
            DecideError::Output {
                reason: "broken pipe".to_string(),
            },
        ];
        for error in &failures {
            assert_eq!(
                error.exit_code(),
                1,
                "{error} should be a failed evaluation"
            );
        }
    }

    #[test]
    fn a_message_names_the_thing_that_is_wrong() {
        let error = DecideError::PathNotFound {
            flag: "--select",
            path: "answers.urgency.noul".to_string(),
            problem: "no key \"urgency\"".to_string(),
        };

        let rendered = error.to_string();
        assert!(rendered.contains("--select"), "{rendered}");
        assert!(rendered.contains("answers.urgency.noul"), "{rendered}");
        assert!(rendered.contains("urgency"), "{rendered}");
    }

    #[test]
    fn a_transport_failure_says_how_many_attempts_it_made() {
        let one = DecideError::Transport {
            reason: "connection refused".to_string(),
            attempts: 1,
        };
        let three = DecideError::Transport {
            reason: "connection refused".to_string(),
            attempts: 3,
        };

        assert!(one.to_string().contains("after 1 attempt:"), "{one}");
        assert!(three.to_string().contains("after 3 attempts:"), "{three}");
    }

    #[test]
    fn an_unreadable_file_names_the_path_and_the_os_error() {
        let error = DecideError::UnreadableFile {
            path: PathBuf::from("/tmp/nope.txt"),
            reason: "No such file or directory (os error 2)".to_string(),
        };

        let rendered = error.to_string();
        assert!(rendered.contains("/tmp/nope.txt"), "{rendered}");
        assert!(rendered.contains("os error 2"), "{rendered}");
    }
}
