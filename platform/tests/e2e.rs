//! End-to-end tests: the real simulators and the real platform, as separate processes, driven
//! over HTTP the way a GCS drives them.
//!
//! Run them with `cargo test`. The simulators must not be running already: each test starts
//! its own copy. If they haven't been built yet, the first test builds them.

mod common;

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::time::sleep;

use common::{Camera, Reply, platform, simulator, start, turn};

// ─── One scenario for every camera ───────────────────────────────────────────────────────

/// The same scenario for every camera. It takes every limit from the camera's own
/// description, so nothing in it depends on which camera is attached. That's the assignment's
/// main requirement: the GCS can treat all cameras the same.
async fn common_scenario(camera: Camera) {
    let setup = start(camera).await;
    let api = &setup.api;
    let payload = api.payload().await;
    let properties = &payload["properties"];
    let minimum = properties["zoom"]["minimum"]
        .as_f64()
        .expect("zoom has a minimum");
    let maximum = properties["zoom"]["maximum"]
        .as_f64()
        .expect("zoom has a maximum");

    // Absolute zoom, then one step each way.
    expect_value(api.set("zoom", json!(4)).await, json!(4.0));
    expect_zoom_after_step(api.step("in").await, 5.0);
    expect_zoom_after_step(api.step("out").await, 4.0);

    // At either end of the range a step changes nothing.
    expect_value(api.set("zoom", json!(maximum)).await, json!(maximum));
    expect_zoom_after_step(api.step("in").await, maximum);
    expect_value(api.set("zoom", json!(minimum)).await, json!(minimum));
    expect_zoom_after_step(api.step("out").await, minimum);

    // Every sensor and every palette this camera offers.
    for sensor in options(&properties["sensor"]) {
        expect_value(api.set("sensor", json!(sensor)).await, json!(sensor));
    }
    for palette in options(&properties["thermal_palette"]) {
        expect_value(
            api.set("thermal_palette", json!(palette)).await,
            json!(palette),
        );
    }

    // Reads come from the cache, which every write keeps up to date.
    assert_eq!(api.value("zoom").await, json!(minimum));

    // Mistakes are refused before anything reaches the camera, each with its own code.
    let too_far = api.set("zoom", json!(maximum + 1.0)).await;
    let error = expect_error(too_far, 422, "out_of_range");
    assert_eq!(error["details"]["maximum"], json!(maximum));
    let sepia = api.set("thermal_palette", json!("sepia")).await;
    let error = expect_error(sepia, 422, "unsupported_value");
    assert_eq!(
        error["details"]["options"],
        properties["thermal_palette"]["options"]
    );
    expect_error(api.set("zoom", json!("ten")).await, 400, "invalid_request");
    let typo = api
        .put("/v1/payload/properties/zoom", json!({ "valeu": 4 }))
        .await;
    expect_error(typo, 400, "invalid_request");
    expect_error(api.step("sideways").await, 422, "unsupported_value");
    let no_direction = api.post("/v1/payload/actions/zoom_step", json!({})).await;
    expect_error(no_direction, 400, "invalid_request");
    expect_error(api.set("focus", json!(1)).await, 404, "not_found");
    expect_error(
        api.get("/v1/payload/properties/focus").await,
        404,
        "not_found",
    );
    let photo = api.post("/v1/payload/actions/take_photo", json!({})).await;
    expect_error(photo, 404, "not_found");
}

#[tokio::test]
async fn common_scenario_on_the_bx20() {
    common_scenario(Camera::Bx20).await;
}

#[tokio::test]
async fn common_scenario_on_the_vlr10() {
    common_scenario(Camera::Vlr10).await;
}

#[tokio::test]
async fn common_scenario_on_the_lc30t() {
    common_scenario(Camera::Lc30t).await;
}

// ─── What sets the cameras apart ─────────────────────────────────────────────────────────
// The differences show up only in each camera's description and in how values are rounded,
// as the README's "Camera vocabulary" table says.

#[tokio::test]
async fn the_bx20_is_detected_and_zooms_in_tenths() {
    let setup = start(Camera::Bx20).await;
    let payload = setup.api.payload().await;
    let identity = json!({
        "kind": "camera", "vendor": "Brightmere", "model": "BX-20", "firmware": "1.0.0.0",
        "driver": "mavlink", "profile": "bx20", "uri": "mavlink://127.0.0.1:14580",
    });
    assert_eq!(payload["identity"], identity);
    assert_eq!(
        descriptor(&payload["properties"]["zoom"]),
        zoom_descriptor(20.0, 0.1)
    );
    assert_eq!(options(&payload["properties"]["thermal_palette"]).len(), 4);
    expect_value(setup.api.set("zoom", json!(4.04)).await, json!(4.0));
    expect_value(setup.api.set("zoom", json!(7.06)).await, json!(7.1));
}

#[tokio::test]
async fn the_vlr10_is_detected_and_zooms_in_whole_steps() {
    let setup = start(Camera::Vlr10).await;
    let payload = setup.api.payload().await;
    let identity = json!({
        "kind": "camera", "vendor": "Vallerin", "model": "VLR-10", "firmware": "2.3",
        "driver": "mavlink", "profile": "vlr10", "uri": "mavlink://127.0.0.1:14581",
    });
    assert_eq!(payload["identity"], identity);
    assert_eq!(
        descriptor(&payload["properties"]["zoom"]),
        zoom_descriptor(10.0, 1.0)
    );
    assert_eq!(options(&payload["properties"]["thermal_palette"]).len(), 4);
    expect_value(setup.api.set("zoom", json!(4.4)).await, json!(4.0));
    expect_value(setup.api.set("zoom", json!(4.6)).await, json!(5.0));
}

#[tokio::test]
async fn the_lc30t_has_no_firmware_and_ten_palettes() {
    let setup = start(Camera::Lc30t).await;
    let payload = setup.api.payload().await;
    // The LC-30T can't say who it is, so its driver fills in what it can.
    let identity = json!({
        "kind": "camera", "vendor": "Lucen", "model": "LC-30T", "firmware": null,
        "driver": "lc30t", "profile": null, "uri": "lc30t://127.0.0.1:9003",
    });
    assert_eq!(payload["identity"], identity);
    assert_eq!(
        descriptor(&payload["properties"]["zoom"]),
        zoom_descriptor(30.0, 0.1)
    );
    assert_eq!(options(&payload["properties"]["thermal_palette"]).len(), 10);
    // `lava` is refused on the MAVLink cameras (see `common_scenario`), but the LC-30T has it.
    expect_value(
        setup.api.set("thermal_palette", json!("lava")).await,
        json!("lava"),
    );
}

#[tokio::test]
async fn a_forced_profile_is_used() {
    let _turn = turn(Camera::Vlr10).await;
    // Rust: `_simulator`, not just `_`. A value bound to `_` is dropped on the spot, which
    // would stop the simulator straight away; `_simulator` lives to the end of the test.
    let _simulator = simulator(Camera::Vlr10).await;
    let (_platform, api) = platform("mavlink://127.0.0.1:14581?profile=vlr10");
    api.wait_for("online").await;
    assert_eq!(api.payload().await["identity"]["profile"], "vlr10");
}

// ─── Real-time scenarios ─────────────────────────────────────────────────────────────────

/// The camera stops, so the platform marks it offline, refuses writes and still answers reads
/// from the cache. The camera starts again, and the platform comes back online on its own
/// and shows the camera's new state.
async fn survives_a_camera_restart(camera: Camera) {
    let mut setup = start(camera).await;
    let api = &setup.api;
    expect_value(api.set("zoom", json!(7)).await, json!(7.0));

    setup.simulator.stop();
    let noticed = api.wait_for("offline").await;
    // Offline after 3 s without a sign of life, checked every second (README, "Timing and
    // liveness"). The camera's last sign of life can be up to a second before it stopped.
    assert!(
        noticed <= Duration::from_secs(5),
        "offline only after {noticed:?}"
    );
    expect_error(api.set("zoom", json!(4)).await, 503, "payload_offline");
    assert_eq!(api.value("zoom").await, json!(7.0));

    // Rust: assigning to the field drops the old, already stopped, simulator.
    setup.simulator = simulator(camera).await;
    api.wait_for("online").await;
    // A camera that restarted is back at its power-on zoom of 1x, and the platform shows it.
    api.wait_for_value("zoom", json!(1.0)).await;
    expect_value(api.set("zoom", json!(4)).await, json!(4.0));
}

#[tokio::test]
async fn the_bx20_can_restart() {
    survives_a_camera_restart(Camera::Bx20).await;
}

#[tokio::test]
async fn the_vlr10_can_restart() {
    survives_a_camera_restart(Camera::Vlr10).await;
}

#[tokio::test]
async fn the_lc30t_can_restart() {
    survives_a_camera_restart(Camera::Lc30t).await;
}

/// The platform may start before its camera; the real LC-30T takes about 8 s to boot. Until
/// the camera answers, the platform says `connecting` and refuses writes.
async fn waits_for_a_camera_that_starts_late(camera: Camera) {
    let _turn = turn(camera).await;
    let (_platform, api) = platform(&camera.uri(camera.port()));
    api.wait_for("connecting").await;
    expect_error(api.set("zoom", json!(4)).await, 503, "payload_offline");
    let _simulator = simulator(camera).await;
    api.wait_for("online").await;
    expect_value(api.set("zoom", json!(4)).await, json!(4.0));
}

#[tokio::test]
async fn waits_for_a_bx20_that_starts_late() {
    waits_for_a_camera_that_starts_late(Camera::Bx20).await;
}

#[tokio::test]
async fn waits_for_an_lc30t_that_starts_late() {
    waits_for_a_camera_that_starts_late(Camera::Lc30t).await;
}

/// A wrong URI stops the platform at once, saying what's wrong: retrying can't fix it. None
/// of these needs a camera, because the platform finds the mistake before it sends anything.
#[tokio::test]
async fn a_wrong_uri_stops_the_platform_at_once() {
    let mistakes = [
        (
            "mavlink://127.0.0.1:14580?profile=vlr11",
            "unknown profile `vlr11`",
        ),
        (
            "lc30t://127.0.0.1:9003?interval=100",
            "unknown option `interval`",
        ),
        (
            "visca://127.0.0.1:52381",
            "unknown camera URI scheme `visca`",
        ),
        ("mavlink://127.0.0.1", "needs a host and a port"),
    ];
    for (uri, message) in mistakes {
        let (mut platform, _api) = platform(uri);
        let started = Instant::now();
        let status = loop {
            if let Some(status) = platform.exit_status() {
                break status;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "still running with {uri}"
            );
            sleep(Duration::from_millis(50)).await;
        };
        assert!(!status.success(), "{uri} should fail");
        let output = platform.output();
        assert!(
            output.contains(message),
            "no `{message}` for {uri} in:\n{output}"
        );
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────────────────

/// The options of an enum property, e.g. `["visible", "thermal"]`.
fn options(property: &Value) -> Vec<String> {
    let options = property["options"].as_array().expect("an enum has options");
    options
        .iter()
        .map(|option| option.as_str().unwrap().to_string())
        .collect()
}

/// A property without its current value: only what describes it.
fn descriptor(property: &Value) -> Value {
    let mut descriptor = property.clone();
    let fields = descriptor.as_object_mut().expect("a property is an object");
    fields.remove("value");
    fields.remove("observed_at");
    descriptor
}

/// The descriptor every camera's `zoom` should have, with its own maximum and resolution.
///
/// JSON numbers are compared by type too, so `1.0` here, never `1`: the platform sends floats.
fn zoom_descriptor(maximum: f64, resolution: f64) -> Value {
    json!({
        "type": "number", "unit": "x", "minimum": 1.0, "maximum": maximum,
        "resolution": resolution, "writable": true,
    })
}

/// Checks that a write succeeded and that the camera now reports `value`.
fn expect_value(reply: Reply, value: Value) {
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.body["value"], value, "{}", reply.body);
}

/// Checks that `zoom_step` succeeded and left the zoom at `zoom`.
fn expect_zoom_after_step(reply: Reply, zoom: f64) {
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.body["properties"]["zoom"]["value"], json!(zoom));
}

/// Checks that a request failed with this HTTP status and error `code`, and returns the
/// error for further checks.
fn expect_error(reply: Reply, status: u16, code: &str) -> Value {
    assert_eq!(reply.status, status, "{}", reply.body);
    assert_eq!(reply.body["error"]["code"], code, "{}", reply.body);
    reply.body["error"].clone()
}
