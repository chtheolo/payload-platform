//! Benchmarks: how long each operation takes on each camera, measured the way the GCS sees it
//! (from sending the request to receiving the answer), and what happens to a burst of requests.
//!
//! They print tables instead of checking limits, so plain `cargo test` skips them. Run them
//! one camera at a time, so the cameras don't slow each other down:
//!
//! ```sh
//! cargo test --release --test bench -- --ignored --nocapture --test-threads 1
//! ```
//!
//! Almost all of the time goes to waiting for the camera; the platform's own work takes
//! microseconds. That's why these time whole requests rather than single functions.

mod common;

use std::time::{Duration, Instant};

use serde_json::json;
use tokio::task::JoinSet;

use common::{Api, Camera, start};

/// How many times each operation runs.
const RUNS: usize = 20;
/// How many writes the burst sends at the same moment. The core queues 16 (README, "Timing
/// and liveness"), so some of them should be told `busy`.
const BURST: usize = 30;

#[tokio::test]
#[ignore = "a benchmark; run it with --ignored"]
async fn bx20() {
    benchmark(Camera::Bx20).await;
}

#[tokio::test]
#[ignore = "a benchmark; run it with --ignored"]
async fn vlr10() {
    benchmark(Camera::Vlr10).await;
}

#[tokio::test]
#[ignore = "a benchmark; run it with --ignored"]
async fn lc30t() {
    benchmark(Camera::Lc30t).await;
}

async fn benchmark(camera: Camera) {
    let setup = start(camera).await;
    let api = &setup.api;
    println!("\n{} ({RUNS} runs each, in milliseconds)", camera.name());
    println!(
        "{:<26} {:>7} {:>7} {:>7} {:>7}",
        "operation", "min", "median", "p95", "max"
    );

    // Every write alternates between two values, so each run really changes something.
    // Rust: `async |run| ...` is an async closure: calling it gives a future to `.await`.
    let read = time(async |_| api.get("/v1/payload/properties/zoom").await.status).await;
    report("read zoom (from the cache)", read);
    let zoom = time(async |run| api.set("zoom", json!(either(run, 3, 6))).await.status).await;
    report("set zoom, 3x <-> 6x", zoom);
    let step = time(async |run| api.step(either(run, "in", "out")).await.status).await;
    report("zoom_step", step);
    let sensor = time(async |run| {
        let sensor = either(run, "thermal", "visible");
        api.set("sensor", json!(sensor)).await.status
    })
    .await;
    report("set sensor", sensor);
    let palette = time(async |run| {
        let palette = either(run, "black_hot", "white_hot");
        api.set("thermal_palette", json!(palette)).await.status
    })
    .await;
    report("set thermal_palette", palette);

    burst(api).await;
}

/// Runs `operation` RUNS times and returns how long each run took. Every run must succeed.
///
/// Rust: `AsyncFnMut` is the shape of an async closure that may be called again and again.
async fn time(mut operation: impl AsyncFnMut(usize) -> u16) -> Vec<Duration> {
    let mut durations = Vec::with_capacity(RUNS);
    for run in 0..RUNS {
        let started = Instant::now();
        let status = operation(run).await;
        durations.push(started.elapsed());
        assert_eq!(status, 200, "run {run} failed");
    }
    durations
}

/// Prints one line of the table: the fastest run, the median, the 95th percentile (95% of the
/// runs were at least this fast) and the slowest run.
fn report(operation: &str, mut durations: Vec<Duration>) {
    durations.sort();
    let milliseconds = |index: usize| durations[index].as_secs_f64() * 1000.0;
    let last = durations.len() - 1;
    let p95 = (durations.len() * 95).div_ceil(100) - 1;
    println!(
        "{operation:<26} {:>7.1} {:>7.1} {:>7.1} {:>7.1}",
        milliseconds(0),
        milliseconds(last / 2),
        milliseconds(p95),
        milliseconds(last)
    );
}

/// The first value on even runs, the second on odd ones.
fn either<T>(run: usize, even: T, odd: T) -> T {
    if run.is_multiple_of(2) { even } else { odd }
}

/// Sends BURST zoom writes at the same moment. The core runs one at a time and queues up to
/// 16 more; the others are answered `503 busy` straight away instead of piling up.
async fn burst(api: &Api) {
    let started = Instant::now();
    // Rust: a `JoinSet` runs a group of tasks side by side and collects their results.
    let mut requests = JoinSet::new();
    for run in 0..BURST {
        let api = api.clone();
        requests.spawn(async move { api.set("zoom", json!(either(run, 3, 6))).await });
    }
    let (mut done, mut busy) = (0, 0);
    while let Some(finished) = requests.join_next().await {
        let reply = finished.expect("a request task panicked");
        match (reply.status, reply.body["error"]["code"].as_str()) {
            (200, _) => done += 1,
            (503, Some("busy")) => busy += 1,
            _ => panic!(
                "unexpected answer in the burst: {} {}",
                reply.status, reply.body
            ),
        }
    }
    println!(
        "burst of {BURST} writes at once: {done} done, {busy} answered `busy`, all in {:.0} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
}
