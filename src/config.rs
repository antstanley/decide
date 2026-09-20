//! The stored credential: where it lives, how it is written, and which of the three sources
//! actually supplies it.
//!
//! This is the one file `decide` writes for you, and it holds one secret. It exists because
//! an environment variable is the wrong home for a credential on a developer's machine:
//! `export TYPESAFE_API_KEY=…` in a shell profile leaks into every child process. The
//! variable is still read, and still wins, so a script or a CI runner can override what is
//! stored.
//!
//! The order is fixed and documented in
//! [`docs/cli.md`](../docs/cli.md#where-the-credential-comes-from): the environment, then a
//! file named on the command line, then the store. The store is last because it is a
//! *default* rather than a decision — an explicit flag or variable is the caller saying
//! what to use this time.
//!
//! Nothing here is a general configuration file
//! ([D13](../docs/design.md#d13-one-credential-store-and-still-no-configuration-file)):
//! there is one setting, it is a secret, and it is not parsed — the file *is* the token.

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::error::DecideError;
use crate::input::Stdin;

/// The directory under the configuration root that `decide` owns.
const DIRECTORY: &str = "decide";

/// The file the token is stored in, inside [`DIRECTORY`].
const FILE: &str = "api-key";

/// Which of the three places a credential came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `TYPESAFE_API_KEY`.
    Environment,
    /// A file named by `--api-key-file` or `TYPESAFE_API_KEY_FILE`.
    File(PathBuf),
    /// The credential stored by `decide auth set`.
    Stored(PathBuf),
}

impl Source {
    /// A description for `decide auth status`, which never prints the credential itself.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Environment => "TYPESAFE_API_KEY".to_string(),
            Self::File(path) => format!(
                "the file \"{}\" named by --api-key-file or TYPESAFE_API_KEY_FILE",
                path.display()
            ),
            Self::Stored(path) => format!("the credential stored at \"{}\"", path.display()),
        }
    }
}

/// The path of the store, or `None` when there is no configuration directory for it.
#[must_use]
pub fn path() -> Option<PathBuf> {
    path_from(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

/// The path of the store, given the two variables it is derived from.
///
/// `$XDG_CONFIG_HOME/decide/api-key`, or `$HOME/.config/decide/api-key` when the first is
/// unset or empty — the rule every other tool on the machine already follows, which is why
/// there is no `directories` dependency to derive it.
#[must_use]
pub fn path_from(xdg_config_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    let base = xdg_config_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home.filter(|value| !value.is_empty())
                .map(|home| Path::new(home).join(".config"))
        })?;
    Some(base.join(DIRECTORY).join(FILE))
}

/// Read the stored token, or `None` when nothing has been stored.
pub fn load_at(path: &Path) -> Result<Option<String>, DecideError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        // A store that is not there is not an error: it is the state every user starts in.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(DecideError::CredentialUnreadable {
                path: path.to_path_buf(),
                reason: error.to_string(),
            });
        }
    };

    let key = one_trailing_newline_removed(&text);
    if key.trim().is_empty() {
        // An empty store reads as no store, rather than as a credential that will fail
        // later with a message about a key that is not there.
        return Ok(None);
    }
    Ok(Some(key.to_string()))
}

/// Write the token to the store, readable only by its owner.
pub fn store_at(path: &Path, token: &str) -> Result<(), DecideError> {
    let unwritable = |reason: String| DecideError::CredentialUnwritable {
        path: path.to_path_buf(),
        reason,
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| unwritable(error.to_string()))?;
    }

    // The mode is set as the file is created rather than narrowed afterwards, so the secret
    // is never briefly world-readable.
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| unwritable(error.to_string()))?;
    std::io::Write::write_all(&mut file, format!("{token}\n").as_bytes())
        .map_err(|error| unwritable(error.to_string()))?;
    Ok(())
}

/// Remove the stored token, and say whether there was one.
pub fn clear_at(path: &Path) -> Result<bool, DecideError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(DecideError::CredentialUnwritable {
            path: path.to_path_buf(),
            reason: error.to_string(),
        }),
    }
}

/// Resolve the credential, and say where it came from.
///
/// `stored` is the path of the store, which the caller takes from [`path`] so that the
/// resolution is a function of its arguments rather than of the environment.
pub fn resolve(
    environment: Option<String>,
    file: Option<&Path>,
    stored: Option<&Path>,
) -> Result<(Source, String), DecideError> {
    if let Some(key) = environment.filter(|key| !key.is_empty()) {
        return Ok((Source::Environment, key));
    }

    if let Some(path) = file {
        let key = api_key_file(path)?;
        return Ok((Source::File(path.to_path_buf()), key));
    }

    if let Some(path) = stored
        && let Some(key) = load_at(path)?
    {
        return Ok((Source::Stored(path.to_path_buf()), key));
    }

    Err(DecideError::MissingApiKey)
}

/// Read a credential file, stripping one trailing newline so a file written by `echo` works.
pub fn api_key_file(path: &Path) -> Result<String, DecideError> {
    let text =
        std::fs::read_to_string(path).map_err(|error| DecideError::ApiKeyFileUnreadable {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
    let key = one_trailing_newline_removed(&text);
    if key.trim().is_empty() {
        return Err(DecideError::ApiKeyFileEmpty {
            path: path.to_path_buf(),
        });
    }
    Ok(key.to_string())
}

/// One trailing newline, and the carriage return before it, removed.
fn one_trailing_newline_removed(text: &str) -> &str {
    let key = text.strip_suffix('\n').unwrap_or(text);
    key.strip_suffix('\r').unwrap_or(key)
}

/// Read the token for `decide auth set` from stdin.
///
/// A terminal is refused rather than read: with echo left on, a secret typed at a prompt is
/// printed to the screen and into the scrollback, and turning echo off needs a dependency
/// this program does not have. A pipe is the honest instruction, and it is the same one
/// `gh auth login --with-token` gives.
pub fn read_token<R: Read>(stdin: &mut Stdin<R>) -> Result<String, DecideError> {
    if stdin.is_terminal() {
        return Err(DecideError::TokenFromTerminal);
    }
    let text = stdin
        .read_all()
        .map_err(|error| DecideError::UnreadableFile {
            path: PathBuf::from("-"),
            reason: error.to_string(),
        })?;
    let token = one_trailing_newline_removed(&text);
    if token.trim().is_empty() {
        return Err(DecideError::EmptyToken);
    }
    Ok(token.to_string())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    /// A stdin over in-memory text.
    fn stdin(text: &str, terminal: bool) -> Stdin<Cursor<Vec<u8>>> {
        Stdin::new(Cursor::new(text.as_bytes().to_vec()), terminal)
    }

    /// A path in a temporary directory that does not exist yet.
    fn store() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("nested").join("api-key");
        (dir, path)
    }

    #[test]
    fn the_store_is_under_the_configuration_home_when_there_is_one() {
        let path = path_from(Some(OsStr::new("/xdg")), Some(OsStr::new("/home/ant")));

        assert_eq!(path, Some(PathBuf::from("/xdg/decide/api-key")));
    }

    #[test]
    fn the_store_falls_back_to_dot_config_under_home() {
        let path = path_from(None, Some(OsStr::new("/home/ant")));

        assert_eq!(
            path,
            Some(PathBuf::from("/home/ant/.config/decide/api-key"))
        );
    }

    #[test]
    fn an_empty_configuration_home_is_treated_as_unset() {
        let path = path_from(Some(OsStr::new("")), Some(OsStr::new("/home/ant")));

        assert_eq!(
            path,
            Some(PathBuf::from("/home/ant/.config/decide/api-key"))
        );
    }

    #[test]
    fn with_no_home_at_all_there_is_nowhere_to_store_a_credential() {
        assert_eq!(path_from(None, None), None);
        assert_eq!(path_from(Some(OsStr::new("")), Some(OsStr::new(""))), None);
    }

    #[test]
    fn a_stored_token_is_read_back_after_it_is_written() {
        let (_dir, path) = store();

        store_at(&path, "sekrit-token").expect("the store is writable");

        assert_eq!(
            load_at(&path).expect("the store is readable"),
            Some("sekrit-token".to_string())
        );
    }

    #[test]
    fn the_store_is_created_with_its_parent_directory() {
        let (_dir, path) = store();

        store_at(&path, "sekrit-token").expect("the store is writable");

        assert!(path.exists(), "the parent directory was created too");
    }

    #[cfg(unix)]
    #[test]
    fn the_store_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt as _;
        let (_dir, path) = store();

        store_at(&path, "sekrit-token").expect("the store is writable");

        let mode = std::fs::metadata(&path)
            .expect("the file is there")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "a secret is not world-readable");
    }

    #[test]
    fn a_store_that_is_not_there_reads_as_nothing_stored() {
        let (_dir, path) = store();

        assert_eq!(load_at(&path).expect("nothing to read"), None);
    }

    #[test]
    fn a_store_holding_only_whitespace_reads_as_nothing_stored() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("api-key");
        std::fs::write(&path, "\n").expect("write the store");

        assert_eq!(load_at(&path).expect("readable"), None);
    }

    #[test]
    fn clearing_the_store_says_whether_there_was_anything_to_clear() {
        let (_dir, path) = store();

        assert!(!clear_at(&path).expect("nothing to clear"));

        store_at(&path, "sekrit-token").expect("the store is writable");
        assert!(clear_at(&path).expect("clearing works"));
        assert_eq!(load_at(&path).expect("nothing left"), None);
    }

    #[test]
    fn the_environment_wins_over_everything_else() {
        let (_dir, path) = store();
        store_at(&path, "stored").expect("the store is writable");

        let (source, key) = resolve(
            Some("from-the-environment".to_string()),
            Some(Path::new("/nonexistent/key")),
            Some(&path),
        )
        .expect("the environment supplies it");

        assert_eq!(source, Source::Environment);
        assert_eq!(key, "from-the-environment");
    }

    #[test]
    fn a_named_file_wins_over_the_store() {
        let (_dir, path) = store();
        store_at(&path, "stored").expect("the store is writable");
        let mut file = tempfile::NamedTempFile::new().expect("a temporary file");
        std::io::Write::write_all(&mut file, b"from-the-file\n").expect("write");

        let (source, key) = resolve(None, Some(file.path()), Some(&path)).expect("the file wins");

        assert_eq!(source, Source::File(file.path().to_path_buf()));
        assert_eq!(key, "from-the-file");
    }

    #[test]
    fn the_store_supplies_the_credential_when_nothing_else_does() {
        let (_dir, path) = store();
        store_at(&path, "stored").expect("the store is writable");

        let (source, key) = resolve(None, None, Some(&path)).expect("the store supplies it");

        assert_eq!(source, Source::Stored(path));
        assert_eq!(key, "stored");
    }

    #[test]
    fn an_empty_environment_variable_falls_through_to_the_store() {
        let (_dir, path) = store();
        store_at(&path, "stored").expect("the store is writable");

        let (_source, key) =
            resolve(Some(String::new()), None, Some(&path)).expect("the store supplies it");

        assert_eq!(key, "stored");
    }

    #[test]
    fn with_nothing_anywhere_there_is_no_credential() {
        let (_dir, path) = store();

        let error = resolve(None, None, Some(&path)).expect_err("there is no key");

        assert_eq!(error.exit_code(), 2);
        assert!(error.to_string().contains("decide auth set"), "{error}");
    }

    #[test]
    fn a_named_file_that_cannot_be_read_does_not_fall_through_to_the_store() {
        let (_dir, path) = store();
        store_at(&path, "stored").expect("the store is writable");

        let error = resolve(None, Some(Path::new("/nonexistent/key")), Some(&path))
            .expect_err("a named file is not optional");

        assert!(error.to_string().contains("/nonexistent/key"), "{error}");
    }

    #[test]
    fn a_credential_is_read_from_a_pipe_and_not_from_a_terminal() {
        let mut pipe = stdin("sekrit-token\n", false);
        assert_eq!(
            read_token(&mut pipe).expect("a pipe is fine"),
            "sekrit-token"
        );

        let mut terminal = stdin("sekrit-token\n", true);
        let error = read_token(&mut terminal).expect_err("a terminal is refused");
        assert!(error.to_string().contains("pipe it in"), "{error}");
    }

    #[test]
    fn an_empty_token_is_refused_rather_than_stored() {
        let mut pipe = stdin("  \n", false);

        let error = read_token(&mut pipe).expect_err("nothing to store");

        assert_eq!(error.exit_code(), 2);
        assert!(error.to_string().contains("empty"), "{error}");
    }

    #[test]
    fn a_source_describes_itself_without_revealing_the_credential() {
        assert_eq!(Source::Environment.describe(), "TYPESAFE_API_KEY");

        let described =
            Source::Stored(PathBuf::from("/home/ant/.config/decide/api-key")).describe();
        assert!(described.contains("api-key"), "{described}");
    }
}
