//! Metrics for Prometheus, in the development build only.
//!
//! `cargo run --features prometheus -- --camera ... --metrics-listen 0.0.0.0:9000` serves them
//! at `http://<host>:9000/metrics`, and `monitoring/` has Prometheus and a ready Grafana
//! dashboard. A plain `cargo build` is the production build: it doesn't even download the
//! metrics crates, every function below is empty, and the compiler removes the calls to them.
//!
//! Every `#[cfg(feature = "prometheus")]` in the platform is in this file, so the rest of the
//! code calls these functions the same way in both builds.
//!
//! Rust: `#[cfg(...)]` decides at compile time whether the item or statement after it exists.
//! The compiler only checks the build it's asked for, so check both:
//! `cargo clippy --all-targets` and `cargo clippy --all-targets --features prometheus`.

// In the production build the functions below ignore their arguments.
// Rust: `#![...]` applies to the whole file, and `cfg_attr` applies `allow` in that build only.
#![cfg_attr(not(feature = "prometheus"), allow(unused_variables))]

use std::time::Duration;

use axum::Router;

use crate::model::Identity;

/// The command-line options for metrics, added to the platform's own in main.rs.
///
/// In the production build this struct is empty, so `--metrics-listen` doesn't exist there.
#[derive(clap::Args, Debug)]
pub struct Options {
    /// Serve Prometheus metrics at this address, e.g. 0.0.0.0:9000. Off when not given.
    #[cfg(feature = "prometheus")]
    #[arg(long)]
    metrics_listen: Option<std::net::SocketAddr>,
}

/// Starts serving metrics, if `--metrics-listen` was given. A port that's already taken stops
/// the platform with a clear error, rather than running on without metrics.
pub fn start(options: &Options) -> Result<(), String> {
    #[cfg(feature = "prometheus")]
    if let Some(address) = options.metrics_listen {
        prometheus::start(address)?;
    }
    Ok(())
}

/// Counts and times every HTTP request. In the production build: the router, unchanged.
pub fn instrument(router: Router) -> Router {
    #[cfg(feature = "prometheus")]
    let router = router.layer(axum::middleware::from_fn(prometheus::track_request));
    router
}

/// The camera said who it is. `payload_camera_info` is always 1 and carries the identity in its
/// labels, the usual way to publish text to Prometheus. The dashboard uses it to show camera
/// names instead of addresses.
pub fn camera_identified(identity: &Identity) {
    #[cfg(feature = "prometheus")]
    metrics::gauge!(
        "payload_camera_info",
        "vendor" => identity.vendor.clone(),
        "model" => identity.model.clone(),
        "firmware" => identity.firmware.clone().unwrap_or_else(|| "unknown".into()),
        "driver" => identity.driver.clone(),
        "profile" => identity.profile.clone().unwrap_or_else(|| "none".into()),
    )
    .set(1.0);
}

/// The result of the liveness check the core makes every second.
pub fn camera_status(online: bool, last_heard: Option<Duration>) {
    #[cfg(feature = "prometheus")]
    {
        metrics::gauge!("payload_camera_online").set(if online { 1.0 } else { 0.0 });
        if let Some(age) = last_heard {
            metrics::gauge!("payload_camera_last_heard_seconds").set(age.as_secs_f64());
        }
    }
}

/// One poll of every property, and whether it worked.
pub fn poll_done(ok: bool) {
    #[cfg(feature = "prometheus")]
    metrics::counter!("payload_polls_total", "outcome" => if ok { "ok" } else { "failed" })
        .increment(1);
}

/// How many jobs are waiting in the queue, out of 16.
pub fn queue_depth(depth: usize) {
    #[cfg(feature = "prometheus")]
    metrics::gauge!("payload_queue_depth").set(depth as f64);
}

/// One job done by the core, `kind` being `write` or `invoke`: how long the camera work took,
/// without the time the job waited in the queue.
pub fn job_done(kind: &'static str, took: Duration) {
    #[cfg(feature = "prometheus")]
    metrics::histogram!("payload_job_duration_seconds", "kind" => kind).record(took.as_secs_f64());
}

/// An error sent to the GCS, by its code (README, "Errors").
pub fn error_sent(code: &'static str) {
    #[cfg(feature = "prometheus")]
    metrics::counter!("payload_errors_total", "code" => code).increment(1);
}

/// A request sent to the camera again: its answer didn't come in time, or, on the LC-30T, the
/// read-back after a write still differed.
pub fn camera_resend() {
    #[cfg(feature = "prometheus")]
    metrics::counter!("payload_camera_resends_total").increment(1);
}

/// A request to the camera that got no answer after every attempt.
pub fn camera_unanswered() {
    #[cfg(feature = "prometheus")]
    metrics::counter!("payload_camera_unanswered_total").increment(1);
}

/// What only the development build contains: the exporter that serves the metrics, and the
/// HTTP middleware.
#[cfg(feature = "prometheus")]
mod prometheus {
    use std::net::SocketAddr;
    use std::time::Instant;

    use axum::extract::{MatchedPath, Request};
    use axum::middleware::Next;
    use axum::response::Response;
    use metrics::{Unit, describe_counter, describe_gauge, describe_histogram};
    use metrics_exporter_prometheus::{Matcher, PrometheusBuilder};
    use tracing::info;

    /// The upper limits of the duration histograms' buckets, in seconds: from a read answered
    /// from the cache (well under 1 ms) to the 5 s request deadline. An LC-30T write takes
    /// about 0.1 s, a MAVLink one about 1 ms.
    const DURATION_BUCKETS: [f64; 12] = [
        0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0,
    ];

    /// Installs the exporter: from now on, metrics are recorded, and served at
    /// `http://<address>/metrics` by a task of its own.
    pub fn start(address: SocketAddr) -> Result<(), String> {
        PrometheusBuilder::new()
            .with_http_listener(address)
            .set_buckets_for_metric(
                Matcher::Suffix("duration_seconds".into()),
                &DURATION_BUCKETS,
            )
            .and_then(PrometheusBuilder::install)
            .map_err(|error| format!("can't serve metrics on {address}: {error}"))?;
        describe();
        // A metric only appears once something records it. These start at zero, so a graph of
        // them shows a flat line rather than "no data" while nothing goes wrong.
        metrics::counter!("payload_camera_resends_total").increment(0);
        metrics::counter!("payload_camera_unanswered_total").increment(0);
        metrics::gauge!("payload_queue_depth").set(0.0);
        metrics::gauge!("payload_camera_online").set(0.0);
        info!("Prometheus metrics at http://{address}/metrics");
        Ok(())
    }

    /// The help text Prometheus shows next to each metric.
    fn describe() {
        describe_counter!(
            "payload_http_requests_total",
            "HTTP requests from the GCS, by method, route and status"
        );
        describe_histogram!(
            "payload_http_request_duration_seconds",
            Unit::Seconds,
            "How long HTTP requests took, as the GCS sees it"
        );
        describe_counter!("payload_errors_total", "Errors sent to the GCS, by code");
        describe_histogram!(
            "payload_job_duration_seconds",
            Unit::Seconds,
            "How long the camera work of a write or an action took, without the queue"
        );
        describe_gauge!(
            "payload_queue_depth",
            "Jobs waiting in the queue, out of 16"
        );
        describe_gauge!(
            "payload_camera_online",
            "1 while the camera is online, else 0"
        );
        describe_gauge!(
            "payload_camera_last_heard_seconds",
            Unit::Seconds,
            "How long ago the camera was last heard from"
        );
        describe_gauge!(
            "payload_camera_info",
            "The camera's identity, in the labels"
        );
        describe_counter!("payload_polls_total", "Polls of the camera, by outcome");
        describe_counter!(
            "payload_camera_resends_total",
            "Requests sent to the camera again"
        );
        describe_counter!(
            "payload_camera_unanswered_total",
            "Requests the camera never answered"
        );
    }

    /// Axum middleware, which runs around every request: counts it by route and status, and
    /// records how long it took.
    pub async fn track_request(request: Request, next: Next) -> Response {
        let started = Instant::now();
        let method = request.method().to_string();
        // The route's pattern, such as /v1/payload/properties/{id}, rather than the real path:
        // one series per route, not one per property id, and none for random paths.
        let route = request
            .extensions()
            .get::<MatchedPath>()
            .map_or_else(|| "unmatched".to_string(), |path| path.as_str().to_string());
        let response = next.run(request).await;
        let status = response.status().as_u16().to_string();
        metrics::counter!(
            "payload_http_requests_total",
            "method" => method.clone(),
            "route" => route.clone(),
            "status" => status,
        )
        .increment(1);
        metrics::histogram!(
            "payload_http_request_duration_seconds",
            "method" => method,
            "route" => route,
        )
        .record(started.elapsed().as_secs_f64());
        response
    }
}
