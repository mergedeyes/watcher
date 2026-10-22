use std::time::Duration;
use watcher_core::Termination;
use watcher_exec::{CommandLine, ExecOptions, run};

fn sh(script: &str) -> CommandLine {
    CommandLine::parse(script, true).unwrap()
}

fn opts(timeout_ms: u64) -> ExecOptions {
    ExecOptions {
        timeout: Duration::from_millis(timeout_ms),
        ..ExecOptions::default()
    }
}

#[tokio::test]
async fn captures_output_and_exit_code() {
    let exec = run(&sh("echo 'WARN | a=1'; echo oops >&2; exit 1"), &opts(5000)).await;
    assert_eq!(exec.output.termination, Termination::Exited(1));
    assert_eq!(exec.output.stdout, "WARN | a=1\n");
    assert_eq!(exec.output.stderr, "oops\n");
    assert!(!exec.stdout_truncated);
}

#[tokio::test]
async fn runs_argv_without_a_shell() {
    let cmd = CommandLine::parse("echo '$HOME' \"a b\"", false).unwrap();
    let exec = run(&cmd, &opts(5000)).await;
    assert_eq!(exec.output.stdout, "$HOME a b\n");
}

#[tokio::test]
async fn missing_program_fails_to_start() {
    let cmd = CommandLine::parse("/nonexistent/check_foo", false).unwrap();
    let exec = run(&cmd, &opts(5000)).await;
    let Termination::Failed(msg) = exec.output.termination else {
        panic!("{:?}", exec.output.termination)
    };
    assert!(msg.starts_with("/nonexistent/check_foo: "), "{msg}");
}

#[tokio::test]
async fn timeout_kills_the_whole_group() {
    // The background sleep holds stdout open; it must die with the group.
    let exec = run(&sh("sleep 30 & echo started; sleep 30"), &opts(300)).await;
    assert_eq!(
        exec.output.termination,
        Termination::TimedOut(Duration::from_millis(300))
    );
    assert_eq!(exec.output.stdout, "started\n");
    assert!(
        exec.duration < Duration::from_secs(5),
        "{:?}",
        exec.duration
    );
}

#[tokio::test]
async fn leftover_children_are_killed_on_exit() {
    let exec = run(&sh("sleep 30 & echo done"), &opts(10_000)).await;
    assert_eq!(exec.output.termination, Termination::Exited(0));
    assert_eq!(exec.output.stdout, "done\n");
    assert!(
        exec.duration < Duration::from_secs(5),
        "{:?}",
        exec.duration
    );
}

#[tokio::test]
async fn large_output_is_capped_but_drained() {
    let opts = ExecOptions {
        output_limit: 1000,
        ..opts(10_000)
    };
    // 1 MiB is far more than a pipe buffer; the plugin must not block.
    let exec = run(&sh("head -c 1048576 /dev/zero | tr '\\0' x; exit 0"), &opts).await;
    assert_eq!(exec.output.termination, Termination::Exited(0));
    assert_eq!(exec.output.stdout.len(), 1000);
    assert!(exec.stdout_truncated);
}

#[tokio::test]
async fn signals_are_reported() {
    let exec = run(&sh("kill -TERM $$"), &opts(5000)).await;
    assert_eq!(exec.output.termination, Termination::Signaled(15));
}
