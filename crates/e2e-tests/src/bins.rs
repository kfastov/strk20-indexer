//! Locate, build (once) and spawn the REAL binaries — the acceptance test
//! runs `strk20` and `strk20-sync` as separate processes over real HTTP
//! (spec §10.3), never in-process shortcuts.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Once;

static BUILD: Once = Once::new();

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}

fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target"))
}

pub fn ensure_built() {
    BUILD.call_once(|| {
        let status = Command::new(env!("CARGO"))
            .args(["build", "-p", "strk20-indexerd", "-p", "strk20-client"])
            .current_dir(workspace_root())
            .status()
            .expect("spawn cargo build");
        assert!(status.success(), "building binaries failed");
    });
}

pub fn bin(name: &str) -> PathBuf {
    target_dir().join("debug").join(name)
}

/// Child process killed on drop; stdout/stderr redirected to files so the
/// server-side scan can inspect them.
pub struct ChildGuard {
    pub child: Child,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn spawn_with_logs(mut cmd: Command, log_dir: &std::path::Path, tag: &str) -> ChildGuard {
    let stdout_path = log_dir.join(format!("{tag}.stdout.log"));
    let stderr_path = log_dir.join(format!("{tag}.stderr.log"));
    let out = std::fs::File::create(&stdout_path).expect("create stdout log");
    let err = std::fs::File::create(&stderr_path).expect("create stderr log");
    isolate_logging(&mut cmd);
    let child = cmd
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()
        .expect("spawn child");
    ChildGuard {
        child,
        stdout_path,
        stderr_path,
    }
}

/// Wait for the port the child actually bound. Reserving then releasing a
/// "free" port before spawning is a race between parallel tests and services.
pub async fn listening_port(child: &ChildGuard) -> u16 {
    for _ in 0..200 {
        let log=std::fs::read_to_string(&child.stderr_path).unwrap_or_default();
        if let Some(line)=log.lines().find(|line|line.contains("http server listening")) {
            if let Some(port)=line.split("addr=127.0.0.1:").nth(1).and_then(|s|s.split_whitespace().next()).and_then(|s|s.parse().ok()) {return port;}
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("child never announced a bound port: {}",std::fs::read_to_string(&child.stderr_path).unwrap_or_default());
}

/// Run a short-lived command, capture stdout, assert exit status.
pub fn run_capture(mut cmd: Command, expect_success: bool) -> (String, String, bool) {
    isolate_logging(&mut cmd);
    let out = cmd.output().expect("run command");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if expect_success && !out.status.success() {
        panic!("command failed\nstdout:\n{stdout}\nstderr:\n{stderr}");
    }
    (stdout, stderr, out.status.success())
}

/// Tests choose their own logging. An inherited RUST_LOG must not erase the
/// progress lines a fixture asserts; explicit per-command settings still win.
fn isolate_logging(cmd: &mut Command) {
    if !cmd.get_envs().any(|(key, _)| key == "RUST_LOG") {
        cmd.env("RUST_LOG", "info");
    }
}
