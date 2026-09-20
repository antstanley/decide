//! The binary: build [`Io`] from the process streams, parse, run, report, and return an
//! exit code. Nothing else lives here.
//!
//! clap exits the process itself on a usage error (exit `2`, message on stderr) and on
//! `--help` and `--version` (exit `0`, on stdout). That is wanted: the usage text belongs
//! to the caller who got the invocation wrong, and it is clap's to write.

use std::io::{IsTerminal as _, Write as _};
use std::process::ExitCode;

use clap::Parser as _;

use decide::{Cli, Io, Stdin};

/// Parse the invocation, run it, and turn the result into an exit code.
fn main() -> ExitCode {
    let cli = Cli::parse();

    let stdin = std::io::stdin();
    let terminal = stdin.is_terminal();
    let mut io = Io {
        stdin: Stdin::new(stdin, terminal),
        stdout: std::io::stdout(),
        stderr: std::io::stderr(),
    };

    match decide::run(&cli, &mut io) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Let stderr own formatting that does not fit `Display` rather than teaching
            // the error type about writers.
            let _ = writeln!(io.stderr, "decide: {error}");
            let _ = io.stderr.flush();
            ExitCode::from(error.exit_code())
        }
    }
}
