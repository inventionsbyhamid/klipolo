//! `lower`: the libc-only lowering every speech worker uses, whisper's through
//! `klipolo_platform::process::lower_self` and klipolo-asr directly. Linux (the gate): the test re-runs itself as a child
//! with `KLIPOLO_LOWER_CHILD=1`; the child lowers its test thread and prints that thread's nice and policy, so the
//! parent's own priority is never changed.
#![cfg(target_os = "linux")]

use klipolo_speech_wire::lower::{ceiling, lower, LOWERED_NICE};

/// The calling thread's `/proc/thread-self/stat` fields after the command's name (which may hold spaces): field 3 on,
/// so nice (19) and policy (41) are 16 and 38.
fn fields() -> Vec<String> {
    let stat = std::fs::read_to_string("/proc/thread-self/stat").unwrap();
    stat[stat.rfind(')').unwrap() + 2..]
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_lowered_worker_runs_at_nice_10_and_sched_batch() {
    if std::env::var_os("KLIPOLO_LOWER_CHILD").is_some() {
        let before: i32 = fields()[16].parse().unwrap();
        ceiling();
        lower().unwrap();
        let after = fields();
        println!(
            "lowered: before {before} nice {} policy {}",
            after[16], after[38]
        );
        return;
    }
    let me = std::env::current_exe().unwrap();
    let out = std::process::Command::new(me)
        .args([
            "--exact",
            "a_lowered_worker_runs_at_nice_10_and_sched_batch",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("KLIPOLO_LOWER_CHILD", "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    let line = stdout
        .lines()
        .find_map(|l| l.split_once("lowered: ").map(|(_, rest)| rest))
        .unwrap_or_else(|| panic!("the child said nothing: {stdout}"));
    let words: Vec<&str> = line.split_whitespace().collect();
    let (before, nice, policy): (i32, i32, &str) = (
        words[1].parse().unwrap(),
        words[3].parse().unwrap(),
        words[5],
    );
    assert_eq!(nice, before.max(LOWERED_NICE), "nice 10, or nicer");
    assert_eq!(policy, "3", "SCHED_BATCH, never SCHED_IDLE (5)");
}
