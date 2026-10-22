#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use orq_core::ipc::{self, DaemonRequest, DaemonResponse};
use orq_core::{CoreError, Step, VerifyRequest};
use orq_daemon::{serve, DaemonOptions};
use tempfile::TempDir;
use tokio::task::JoinHandle;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == "target" {
            continue;
        }
        let dest = to.join(&name);
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c"])
        .arg("commit.gpgsign=false")
        .args(args)
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn fixture() -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/multi");
    copy_dir(&src, dir.path());
    std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    let root = dir.path().canonicalize().unwrap();
    (dir, root)
}

async fn request(root: &Path, req: DaemonRequest) -> Result<Option<DaemonResponse>, CoreError> {
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || ipc::request(&root, &req))
        .await
        .unwrap()
}

fn start(root: &Path, options: DaemonOptions) -> JoinHandle<Result<(), CoreError>> {
    let root = root.to_path_buf();
    tokio::spawn(async move { serve(&root, options).await })
}

async fn wait_ready(root: &Path) {
    for _ in 0..200 {
        if let Ok(Some(DaemonResponse::Status(_))) = request(root, DaemonRequest::Status).await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("daemon did not come up");
}

fn verify_request(root: &Path) -> DaemonRequest {
    DaemonRequest::Verify(VerifyRequest {
        root: root.to_path_buf(),
        up_to: Step::Check,
        use_cache: false,
        ..VerifyRequest::default()
    })
}

async fn stop(root: &Path, handle: JoinHandle<Result<(), CoreError>>) {
    let answer = request(root, DaemonRequest::Stop).await.unwrap();
    assert_eq!(answer, Some(DaemonResponse::Stopping));
    handle.await.unwrap().unwrap();
    assert!(!ipc::socket_path(root).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn status_verify_and_stop() {
    let (_dir, root) = fixture();
    let handle = start(&root, DaemonOptions::default());
    wait_ready(&root).await;

    match request(&root, DaemonRequest::Status).await.unwrap() {
        Some(DaemonResponse::Status(status)) => {
            assert_eq!(status.root, root);
            assert_eq!(status.pid, std::process::id());
        }
        other => panic!("unexpected {other:?}"),
    }

    // A change in leaf: only leaf is affected, and it passes.
    let lib = root.join("crates/leaf/src/lib.rs");
    let original = std::fs::read_to_string(&lib).unwrap();
    std::fs::write(&lib, format!("{original}\n// touched\n")).unwrap();
    match request(&root, verify_request(&root)).await.unwrap() {
        Some(DaemonResponse::Verify(result)) => {
            assert!(result.outcome.is_pass(), "{result:?}");
            let names: Vec<_> = result
                .impact
                .affected()
                .into_iter()
                .map(|c| c.name)
                .collect();
            assert_eq!(names, vec!["leaf".to_string()]);
        }
        other => panic!("unexpected {other:?}"),
    }

    // A compile error in leaf fails at check.
    std::fs::write(
        &lib,
        format!("{original}\nfn broken() -> u32 {{ \"x\" }}\n"),
    )
    .unwrap();
    match request(&root, verify_request(&root)).await.unwrap() {
        Some(DaemonResponse::Verify(result)) => {
            assert!(!result.outcome.is_pass(), "{result:?}");
            assert_eq!(
                result.outcome,
                orq_core::Outcome::Fail {
                    failed_step: Step::Check
                }
            );
        }
        other => panic!("unexpected {other:?}"),
    }

    stop(&root, handle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn idle_timeout_shuts_down_and_cleans_up() {
    let (_dir, root) = fixture();
    let options = DaemonOptions {
        idle_timeout: Duration::from_millis(500),
        ..DaemonOptions::default()
    };
    let handle = start(&root, options);
    let finished = tokio::time::timeout(Duration::from_secs(20), handle)
        .await
        .expect("serve should exit on its own");
    finished.unwrap().unwrap();
    assert!(!ipc::socket_path(&root).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn second_serve_reports_already_running() {
    let (_dir, root) = fixture();
    let handle = start(&root, DaemonOptions::default());
    wait_ready(&root).await;

    let err = serve(&root, DaemonOptions::default()).await.unwrap_err();
    assert!(err.to_string().contains("already running"), "{err}");
    // The first daemon is unaffected.
    assert!(matches!(
        request(&root, DaemonRequest::Status).await.unwrap(),
        Some(DaemonResponse::Status(_))
    ));
    stop(&root, handle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_socket_is_replaced() {
    let (_dir, root) = fixture();
    let socket = ipc::socket_path(&root);
    ipc::prepare_socket_dir(&socket).unwrap();
    // Leave a dead socket file behind.
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    assert!(socket.exists());

    let handle = start(&root, DaemonOptions::default());
    wait_ready(&root).await;
    stop(&root, handle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn socket_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, root) = fixture();
    let handle = start(&root, DaemonOptions::default());
    wait_ready(&root).await;
    let mode = std::fs::metadata(ipc::socket_path(&root))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    stop(&root, handle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_request_is_dropped_and_daemon_survives() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (_dir, root) = fixture();
    let handle = start(&root, DaemonOptions::default());
    wait_ready(&root).await;

    let mut stream = tokio::net::UnixStream::connect(ipc::socket_path(&root))
        .await
        .unwrap();
    let chunk = vec![b'a'; 64 * 1024];
    // 2 MiB without a newline; the daemon may close early, so ignore write errors.
    for _ in 0..32 {
        if stream.write_all(&chunk).await.is_err() {
            break;
        }
    }
    let mut buf = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut buf))
        .await
        .expect("daemon should disconnect");
    // Either a clean EOF or a reset; never a response.
    assert!(read.is_err() || buf.is_empty(), "got a response: {buf:?}");

    assert!(matches!(
        request(&root, DaemonRequest::Status).await.unwrap(),
        Some(DaemonResponse::Status(_))
    ));
    stop(&root, handle).await;
}
