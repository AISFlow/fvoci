//! `oom_score_adj` of a hidden-mode `fvoci-server` child. The child runs with
//! its stdin held open, so it stops at its input read, after its startup
//! setup; the value is read until it is 1000 or five seconds pass.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn oom_score_adj_of_child(args: &[&str]) -> Option<i32> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_fvoci-server"))
        .args(args)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn child mode");
    let path = format!("/proc/{}/oom_score_adj", child.id());
    let deadline = Instant::now() + Duration::from_secs(5);
    let adj = loop {
        let adj = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| text.trim().parse::<i32>().ok());
        if adj == Some(1000) || Instant::now() >= deadline {
            break adj;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let early_exit = child.try_wait().ok().flatten();
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        early_exit.is_none(),
        "{args:?} exited before its input read: {early_exit:?}"
    );
    adj
}
