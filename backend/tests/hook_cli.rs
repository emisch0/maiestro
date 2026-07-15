//! End-to-end test of the `maiestro hook <state> --workspace <id>` CLI path.
//!
//! The app binary doubles as the Claude Code hook helper (see `main()` and
//! CLAUDE.md → "Live per-session status"). Spawned worktrees invoke it on every
//! hook event; it reads the event JSON on stdin and writes a status record to
//! `$MAIESTRO_HOME/status/<id>.json`. This drives the *real* built binary as a
//! subprocess — the exact dispatch path the hook subsystem depends on — with
//! `MAIESTRO_HOME` pointed at a tempdir so nothing touches `~/.maiestro`.

use std::io::Write;
use std::process::{Command, Stdio};

/// Run the binary as `hook <args…>` with `payload` on stdin and `MAIESTRO_HOME`
/// set to `home`. Returns once the process exits (asserting success).
fn run_hook(home: &std::path::Path, args: &[&str], payload: &str) {
    let bin = env!("CARGO_BIN_EXE_maiestro");
    let mut child = Command::new(bin)
        .arg("hook")
        .args(args)
        .env("MAIESTRO_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn maiestro hook");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(payload.as_bytes())
        .expect("write payload");
    let status = child.wait().expect("wait for hook");
    assert!(status.success(), "hook exited non-zero: {status:?}");
}

fn read_status(home: &std::path::Path, ws: &str) -> serde_json::Value {
    let path = home.join("status").join(format!("{ws}.json"));
    let data = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("status file {} not written: {e}", path.display()));
    serde_json::from_str(&data).expect("status file is valid JSON")
}

#[test]
fn session_start_writes_running_record() {
    let home = tempfile::tempdir().unwrap();
    run_hook(
        home.path(),
        &["running", "--workspace", "42-add-foo"],
        r#"{"session_id":"abc123","cwd":"/tmp/wt"}"#,
    );

    let rec = read_status(home.path(), "42-add-foo");
    assert_eq!(rec["workspace"], "42-add-foo");
    assert_eq!(rec["state"], "running");
    assert_eq!(rec["session_id"], "abc123");
    assert_eq!(rec["cwd"], "/tmp/wt");
}

#[test]
fn notification_writes_needs_you_with_detail() {
    let home = tempfile::tempdir().unwrap();
    run_hook(
        home.path(),
        &["notification", "--workspace", "7-fix-bug"],
        r#"{"message":"Waiting on your approval"}"#,
    );

    let rec = read_status(home.path(), "7-fix-bug");
    assert_eq!(rec["state"], "needs_you");
    assert_eq!(rec["detail"], "Waiting on your approval");
}

#[test]
fn empty_stdin_still_writes_a_record() {
    // Claude might send nothing / malformed JSON; the helper must still record a
    // status rather than crash or block.
    let home = tempfile::tempdir().unwrap();
    run_hook(home.path(), &["idle", "--workspace", "9-x"], "");

    let rec = read_status(home.path(), "9-x");
    assert_eq!(rec["state"], "idle");
    assert_eq!(rec["workspace"], "9-x");
}

#[test]
fn unsafe_workspace_id_writes_nothing() {
    // A path-traversal workspace id must be rejected — no file escapes the status
    // dir, and the process still exits cleanly.
    let home = tempfile::tempdir().unwrap();
    run_hook(home.path(), &["running", "--workspace", "../evil"], "{}");
    assert!(
        !home.path().join("status").exists()
            || std::fs::read_dir(home.path().join("status"))
                .map(|mut d| d.next().is_none())
                .unwrap_or(true),
        "no status file should be written for an unsafe workspace id"
    );
}
