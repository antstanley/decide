//! The one test that needs the API.
//!
//! Everything else in the suite runs against a hand-rolled server on `127.0.0.1`, which
//! proves the client is consistent with *our understanding* of the API rather than with the
//! API. This one call is the only check on that understanding, so it is `#[ignore]`d rather
//! than required: the rest of the transcript's claims must be checkable by anyone with a
//! clone.
//!
//! ```sh
//! TYPESAFE_API_KEY=… cargo nextest run --run-ignored ignored-only
//! ```

// An integration test is its own crate, which `clippy.toml`'s `allow-expect-in-tests`
// cannot see: as in `src`'s unit tests, a panic here is the assertion mechanism.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::process::{Command, Stdio};

#[test]
#[ignore = "needs TYPESAFE_API_KEY, and reaches api.typesafe.ai"]
fn a_real_noul_call_answers_with_a_probability() {
    let output = Command::new(env!("CARGO_BIN_EXE_decide"))
        .args([
            "noul",
            "does this message convey urgency?",
            "--no-state",
            "--yes",
            "explicitly time-sensitive",
            "--no",
            "no urgency expressed",
            "--value",
        ])
        .stdin(Stdio::null())
        .output()
        .expect("the binary runs");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let probability: f64 = stdout.trim().parse().expect("--value prints one number");
    assert!(
        (0.0..=1.0).contains(&probability),
        "a noul is a probability, and this one was {stdout}"
    );
}
