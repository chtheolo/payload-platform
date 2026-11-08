//! What the platform tells its operators: log files with `--log-dir`, and, in the development
//! build only, Prometheus metrics with `--metrics-listen`.
//!
//! `cargo test` runs the log test; `cargo test --features prometheus` runs the metrics test too.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::time::sleep;

use common::{Camera, start_with};

#[tokio::test]
async fn log_dir_writes_the_logs_to_a_daily_file() {
    // A folder that doesn't exist yet, so this also checks that the platform creates it.
    let folder =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("logs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    let folder_text = folder.to_str().expect("the test folder's path isn't UTF-8");
    let _setup = start_with(Camera::Bx20, &["--log-dir", folder_text]).await;

    // A background thread writes the lines (src/logging.rs), so give it a moment.
    let started = Instant::now();
    loop {
        let logs = read_logs(&folder);
        if logs.contains("connected to Brightmere BX-20") {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "no connect message in {}: {logs:?}",
            folder.display()
        );
        sleep(Duration::from_millis(100)).await;
    }
}

/// The text of every `platform.<date>.log` file in `folder`, or nothing if there's none yet.
fn read_logs(folder: &Path) -> String {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return String::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            name.starts_with("platform.") && name.ends_with(".log")
        })
        .map(|path| std::fs::read_to_string(path).unwrap_or_default())
        .collect()
}

// Rust: `#[cfg(...)]` on a test leaves it out of the build entirely unless the feature is on.
#[cfg(feature = "prometheus")]
#[tokio::test]
async fn metrics_listen_serves_prometheus_metrics() {
    use serde_json::json;

    let listen = format!("127.0.0.1:{}", common::free_port());
    let setup = start_with(Camera::Bx20, &["--metrics-listen", &listen]).await;
    // One write that works, and one that the core refuses.
    assert_eq!(setup.api.set("zoom", json!(4)).await.status, 200);
    assert_eq!(setup.api.set("zoom", json!(25)).await.status, 422);

    let metrics = reqwest::get(format!("http://{listen}/metrics"))
        .await
        .expect("no answer from the metrics port")
        .text()
        .await
        .expect("no metrics in the answer");
    for expected in [
        r#"model="BX-20""#,
        "payload_camera_online 1",
        r#"route="/v1/payload/properties/{id}""#,
        r#"payload_errors_total{code="out_of_range"} 1"#,
        r#"payload_job_duration_seconds_bucket{kind="write""#,
    ] {
        assert!(
            metrics.contains(expected),
            "`{expected}` is missing from:\n{metrics}"
        );
    }
}
