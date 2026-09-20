//! The release machinery: `scripts/changesets.sh` and `install.sh`.
//!
//! Both are programs a release depends on, so both are held to the suite rather than to a
//! reading. They run under `/bin/sh`, which on macOS is bash 3.2 — that is not incidental:
//! it cannot parse a `case` inside a `$( )`, and both scripts are written around that.
//!
//! Nothing here touches the network. The installer is pointed at a socket this file opens,
//! so what it downloads, verifies, and installs is what the test put there.

// An integration test is its own crate, which `clippy.toml`'s `allow-expect-in-tests`
// cannot see: as in `src`'s unit tests, a panic here is the assertion mechanism.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::fs;
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;

/// Where the repository is, so the scripts under test can be found.
fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// A directory laid out like the repository, with a manifest and a seeded changelog.
fn scratch_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temporary directory");
    fs::create_dir_all(dir.path().join("scripts")).expect("scripts/");
    fs::create_dir_all(dir.path().join(".changeset")).expect(".changeset/");
    fs::copy(
        manifest_dir().join("scripts/changesets.sh"),
        dir.path().join("scripts/changesets.sh"),
    )
    .expect("copy the script");
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"decide\"\nversion = \"0.1.0\"\n",
    )
    .expect("write the manifest");
    fs::write(
        dir.path().join("CHANGELOG.md"),
        "# Changelog\n\n## [0.1.0] - 2026-01-01\n\n- The first release.\n",
    )
    .expect("write the changelog");
    dir
}

/// Empty `.changeset` of everything but this directory's own README.
fn clear_changes(dir: &Path) {
    for entry in fs::read_dir(dir.join(".changeset")).expect("readable") {
        let path = entry.expect("an entry").path();
        if path.file_name().is_some_and(|name| name != "README.md") {
            fs::remove_file(path).expect("remove");
        }
    }
}

/// Write a change file.
fn change(dir: &Path, name: &str, body: &str) {
    fs::write(dir.join(".changeset").join(name), body).expect("write a change file");
}

/// A `cargo` that records its arguments and touches the lock, so `apply` can be exercised
/// without a toolchain or a registry. Returns the directory to put on PATH.
fn stub_cargo(dir: &Path, calls: &Path) -> PathBuf {
    let bin = dir.join("stub-bin");
    fs::create_dir_all(&bin).expect("stub bin");
    let script = bin.join("cargo");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >>\"{}\"\ntouch {}\n",
            calls.display(),
            dir.join("Cargo.lock").display()
        ),
    )
    .expect("write the stub");
    let mut perms = fs::metadata(&script).expect("stat").permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        perms.set_mode(0o755);
    }
    fs::set_permissions(&script, perms).expect("chmod");
    bin
}

/// Run a script in a directory, with an environment, and say what it printed.
fn run(script: &Path, args: &[&str], cwd: &Path, env: &[(&str, &str)]) -> Output {
    let mut command = Command::new("sh");
    command.arg(script).args(args).current_dir(cwd).env_clear();
    command.env("PATH", std::env::var("PATH").unwrap_or_default());
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().expect("the script runs")
}

/// One stream of a finished process, as text.
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

// ---- scripts/changesets.sh ------------------------------------------------------------

#[test]
fn a_changeset_bump_is_the_highest_change_type_it_declares() {
    let dir = scratch_repo();
    let script = dir.path().join("scripts/changesets.sh");

    for (files, expected) in [
        (vec![("a.md", "docs")], "0.1.1"),
        (vec![("a.md", "patch")], "0.1.1"),
        (vec![("a.md", "minor"), ("b.md", "patch")], "0.2.0"),
        (vec![("a.md", "patch"), ("b.md", "major")], "1.0.0"),
        (vec![("a.md", "minor"), ("b.md", "major")], "1.0.0"),
    ] {
        clear_changes(dir.path());
        for (name, kind) in &files {
            change(
                dir.path(),
                name,
                &format!("---\ndecide: {kind}\n---\n\nSomething.\n"),
            );
        }

        let output = run(&script, &["next"], dir.path(), &[]);

        assert_eq!(
            output.status.code(),
            Some(0),
            "for {files:?}: {}",
            text(&output.stderr)
        );
        assert_eq!(text(&output.stdout).trim(), expected, "for {files:?}");
    }
}

#[test]
fn a_change_file_that_is_malformed_is_refused_and_nothing_is_computed() {
    let dir = scratch_repo();
    let script = dir.path().join("scripts/changesets.sh");

    for (name, body, expected) in [
        (
            "no-frontmatter.md",
            "just prose\n",
            "the first line of a change file must be ---",
        ),
        (
            "unclosed.md",
            "---\ndecide: minor\n",
            "the frontmatter is not closed",
        ),
        (
            "other-package.md",
            "---\ndecide-cli: minor\n---\n\nx\n",
            "\"decide-cli\" is not a package here",
        ),
        (
            "no-type.md",
            "---\ndecide:\n---\n\nx\n",
            "has no change type",
        ),
        (
            "no-colon.md",
            "---\nminor\n---\n\nx\n",
            "is not a \"package: type\" pair",
        ),
    ] {
        clear_changes(dir.path());
        change(dir.path(), name, body);

        let output = run(&script, &["next"], dir.path(), &[]);

        assert_eq!(output.status.code(), Some(2), "{name} should be refused");
        assert!(
            text(&output.stderr).contains(expected),
            "{name}: {}",
            text(&output.stderr)
        );
        assert!(
            text(&output.stdout).trim().is_empty(),
            "{name} produced a version anyway: {}",
            text(&output.stdout)
        );
    }
}

#[test]
fn apply_bumps_the_manifest_the_lock_and_the_changelog_then_consumes_the_changes() {
    let dir = scratch_repo();
    let script = dir.path().join("scripts/changesets.sh");
    let calls = dir.path().join("cargo-calls.txt");
    let stub = stub_cargo(dir.path(), &calls);
    let path = format!(
        "{}:{}",
        stub.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    change(
        dir.path(),
        "token-store.md",
        "---\ndecide: minor\n---\n\n`decide auth set` stores the token.\n\nAnd it narrows an old store.\n",
    );
    change(
        dir.path(),
        "note.md",
        "---\ndecide: patch\n---\n\nA patch-shaped note.\n",
    );

    let output = run(&script, &["apply"], dir.path(), &[("PATH", &path)]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        text(&output.stderr)
    );
    assert_eq!(text(&output.stdout).trim(), "0.2.0");
    let manifest = fs::read_to_string(dir.path().join("Cargo.toml")).expect("readable");
    assert!(manifest.contains("version = \"0.2.0\""), "{manifest}");
    assert!(
        fs::read_to_string(&calls)
            .expect("the stub was called")
            .contains("update --workspace"),
        "the lock is refreshed through cargo"
    );
    let changelog = fs::read_to_string(dir.path().join("CHANGELOG.md")).expect("readable");
    assert!(
        changelog.starts_with("# Changelog\n\n## [0.2.0] - "),
        "{changelog}"
    );
    assert!(changelog.contains("### Minor"), "{changelog}");
    assert!(changelog.contains("### Patch"), "{changelog}");
    assert!(
        changelog.contains("- `decide auth set` stores the token."),
        "{changelog}"
    );
    assert!(
        changelog.contains("  And it narrows an old store."),
        "a second paragraph is kept, indented: {changelog}"
    );
    assert!(
        changelog.contains("## [0.1.0] - 2026-01-01"),
        "the previous release is still there: {changelog}"
    );
    assert_eq!(
        run(&script, &["pending"], dir.path(), &[]).stdout,
        b"",
        "the change files were consumed"
    );

    // The release body comes from the same section.
    let notes = run(&script, &["notes", "0.2.0"], dir.path(), &[]);
    assert!(
        text(&notes.stdout).contains("### Minor"),
        "{}",
        text(&notes.stdout)
    );
    assert!(
        !text(&notes.stdout).contains("0.1.0"),
        "{}",
        text(&notes.stdout)
    );
}

// ---- install.sh -----------------------------------------------------------------------

/// A directory served over a socket this test owns, one request at a time.
struct Server {
    addr: SocketAddr,
}

impl Server {
    fn serving(dir: PathBuf) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let addr = listener.local_addr().expect("the bound address");
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let body = answer(&stream, &dir);
                let head = format!(
                    "HTTP/1.1 {} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    if body.is_some() { 200 } else { 404 },
                    body.as_ref().map_or(0, Vec::len)
                );
                let _ = stream.write_all(head.as_bytes());
                if let Some(body) = body {
                    let _ = stream.write_all(&body);
                }
            }
        });
        Self { addr }
    }

    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

/// Read one request and return the file it asked for, when there is one.
fn answer(stream: &TcpStream, dir: &Path) -> Option<Vec<u8>> {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut line = String::new();
    reader.read_line(&mut line).expect("the request line");
    let mut parts = line.split_whitespace();
    let _method = parts.next();
    let path = parts
        .next()
        .unwrap_or("/")
        .trim_start_matches('/')
        .to_string();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).expect("a header") == 0 || header == "\r\n" {
            break;
        }
    }
    if path.is_empty() || path.contains("..") {
        return None;
    }
    fs::read(dir.join(path)).ok()
}

/// Stage a release asset: a tarball holding `decide-<version>-<target>/decide`, and its
/// checksum, in a directory a server can hand out.
fn stage_asset(dir: &Path, version: &str, triple: &str) -> (PathBuf, PathBuf) {
    let name = format!("decide-{version}-{triple}");
    let stage = dir.join(&name);
    fs::create_dir_all(&stage).expect("stage");
    let binary = stage.join("decide");
    fs::write(&binary, format!("#!/bin/sh\necho \"decide {version}\"\n")).expect("write");
    let mut perms = fs::metadata(&binary).expect("stat").permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        perms.set_mode(0o755);
    }
    fs::set_permissions(&binary, perms).expect("chmod");

    let archive = dir.join(format!("decide-{triple}.tar.gz"));
    let status = Command::new("tar")
        .args(["-czf"])
        .arg(&archive)
        .arg("-C")
        .arg(dir)
        .arg(&name)
        .status()
        .expect("tar runs");
    assert!(status.success(), "tar failed");
    fs::remove_dir_all(&stage).expect("clean the stage");

    let checksum = Command::new("shasum")
        .args(["-a", "256"])
        .arg(&archive)
        .output()
        .or_else(|_| Command::new("sha256sum").arg(&archive).output())
        .expect("a checksum tool");
    let hash = text(&checksum.stdout)
        .split_whitespace()
        .next()
        .expect("a hash")
        .to_string();
    let checksum_file = dir.join(format!("decide-{triple}.tar.gz.sha256"));
    fs::write(&checksum_file, format!("{hash}  decide-{triple}.tar.gz\n")).expect("write");
    (archive, checksum_file)
}

/// The installer, from the repository under test.
fn installer() -> PathBuf {
    manifest_dir().join("install.sh")
}

#[test]
fn the_installer_downloads_verifies_and_installs_the_binary() {
    let mirror = tempfile::tempdir().expect("a temporary directory");
    let (_archive, _checksum) = stage_asset(mirror.path(), "9.9.9", "aarch64-apple-darwin");
    let server = Server::serving(mirror.path().to_path_buf());
    let install_dir = tempfile::tempdir().expect("a temporary directory");
    let target = install_dir.path().join("bin");

    let output = run(
        &installer(),
        &[],
        &manifest_dir(),
        &[
            ("DECIDE_BASE_URL", &server.url()),
            ("DECIDE_INSTALL_DIR", target.to_str().expect("utf-8")),
            ("DECIDE_TARGET", "aarch64-apple-darwin"),
        ],
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        text(&output.stderr)
    );
    assert!(
        text(&output.stdout).is_empty(),
        "progress goes to stderr: {}",
        text(&output.stdout)
    );
    assert!(
        text(&output.stderr).contains("checksum verified"),
        "{}",
        text(&output.stderr)
    );
    let installed = target.join("decide");
    assert!(installed.is_file(), "the binary was installed");

    let ran = Command::new(&installed)
        .output()
        .expect("the installed binary runs");
    assert_eq!(text(&ran.stdout).trim(), "decide 9.9.9");
}

#[test]
fn the_installer_refuses_an_archive_that_does_not_match_its_checksum() {
    let mirror = tempfile::tempdir().expect("a temporary directory");
    let (archive, _checksum) = stage_asset(mirror.path(), "9.9.9", "aarch64-apple-darwin");
    // One byte of the archive, so the hash it is checked against cannot match.
    let mut bytes = fs::read(&archive).expect("read");
    if let Some(byte) = bytes.get_mut(10) {
        *byte ^= 0xff;
    }
    fs::write(&archive, bytes).expect("write");
    let server = Server::serving(mirror.path().to_path_buf());
    let install_dir = tempfile::tempdir().expect("a temporary directory");
    let target = install_dir.path().join("bin");

    let output = run(
        &installer(),
        &[],
        &manifest_dir(),
        &[
            ("DECIDE_BASE_URL", &server.url()),
            ("DECIDE_INSTALL_DIR", target.to_str().expect("utf-8")),
            ("DECIDE_TARGET", "aarch64-apple-darwin"),
        ],
    );

    assert_eq!(
        output.status.code(),
        Some(1),
        "a tampered archive must fail"
    );
    assert!(
        text(&output.stderr).contains("does not match its checksum"),
        "{}",
        text(&output.stderr)
    );
    assert!(!target.join("decide").exists(), "nothing was installed");
}

#[test]
fn the_installer_says_what_it_cannot_do_rather_than_guessing() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let bin = dir.path().join("bin");
    fs::create_dir_all(&bin).expect("bin");
    // A host that is not macOS or Linux, as far as the installer can tell.
    let uname = bin.join("uname");
    fs::write(&uname, "#!/bin/sh\necho Windows_NT\n").expect("write");
    let mut perms = fs::metadata(&uname).expect("stat").permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        perms.set_mode(0o755);
    }
    fs::set_permissions(&uname, perms).expect("chmod");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = run(&installer(), &[], &manifest_dir(), &[("PATH", &path)]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("releases/latest"),
        "it points at the release page: {}",
        text(&output.stderr)
    );
}
