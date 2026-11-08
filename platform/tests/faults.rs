//! Integration tests over a bad link. A proxy between the platform and the simulator loses
//! chosen datagrams, and each test checks that the platform behaves as the README promises: it
//! resends what is safe to resend, and otherwise tells the GCS exactly what it doesn't know.

mod common;

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mavlink::dialects::common::{MavCmd, MavMessage};
use mavlink::peek_reader::PeekReader;
use mavlink::read_any_msg;
use serde_json::json;

use common::proxy::Direction;
use common::{Camera, start_behind_proxy};

// Each test counts datagrams inside the proxy's rule, which runs in the proxy's own task, and
// reads the count afterwards in the test.
// Rust: that's why the count is an `Arc<AtomicUsize>`. `Arc` lets two owners share it (the
// rule keeps one clone, the test another), and an atomic number can be changed safely from
// another task without a lock.

#[tokio::test]
async fn the_lc30t_resends_a_lost_write() {
    // Each different write is lost the first time it is sent. The read-back then still shows
    // the old value, so the driver sends the write again, and the GCS gets a normal 200.
    let writes = Arc::new(AtomicUsize::new(0));
    let counted = writes.clone();
    let mut seen = HashSet::new();
    let setup = start_behind_proxy(Camera::Lc30t, move |direction, datagram| {
        if direction == Direction::ToCamera && is_lc30t_write(datagram) {
            counted.fetch_add(1, Ordering::Relaxed);
            // `insert` returns true when the frame wasn't in the set yet: lose that copy only.
            return !seen.insert(datagram.to_vec());
        }
        true
    })
    .await;

    let changes = [
        ("zoom", json!(7.0)),
        ("sensor", json!("thermal")),
        ("thermal_palette", json!("lava")),
    ];
    for (property, value) in changes {
        let reply = setup.api.set(property, value.clone()).await;
        assert_eq!(reply.status, 200, "{property}: {}", reply.body);
        assert_eq!(reply.body["value"], value);
    }
    assert_eq!(
        writes.load(Ordering::Relaxed),
        6,
        "each of the 3 writes is sent twice"
    );
}

#[tokio::test]
async fn the_lc30t_reports_a_write_that_never_arrives() {
    // Every write is lost. After 3 attempts the driver gives up and says so with 409
    // `rejected`, and the zoom hasn't moved.
    let writes = Arc::new(AtomicUsize::new(0));
    let counted = writes.clone();
    let setup = start_behind_proxy(Camera::Lc30t, move |direction, datagram| {
        if direction == Direction::ToCamera && is_lc30t_write(datagram) {
            counted.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        true
    })
    .await;

    let reply = setup.api.set("zoom", json!(7)).await;
    assert_eq!(reply.status, 409, "{}", reply.body);
    assert_eq!(reply.body["error"]["code"], "rejected");
    assert_eq!(writes.load(Ordering::Relaxed), 3);
    assert_eq!(setup.api.value("zoom").await, json!(1.0));
}

#[tokio::test]
async fn the_bx20_resends_a_command_whose_answer_is_lost() {
    // The camera's answer to the zoom command is lost once. A BX-20 zoom command names an
    // absolute position, so sending it twice does no harm: the driver resends it, and the
    // GCS gets a normal 200.
    let commands = Arc::new(AtomicUsize::new(0));
    let counted = commands.clone();
    let mut lost_one = false;
    let setup = start_behind_proxy(Camera::Bx20, move |direction, datagram| {
        let Some(message) = mavlink_message(datagram) else {
            return true;
        };
        if direction == Direction::ToCamera && is_zoom_command(&message) {
            counted.fetch_add(1, Ordering::Relaxed);
        }
        if direction == Direction::ToPlatform && is_zoom_answer(&message) && !lost_one {
            lost_one = true;
            return false;
        }
        true
    })
    .await;

    let reply = setup.api.set("zoom", json!(7)).await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.body["value"], json!(7.0));
    assert_eq!(commands.load(Ordering::Relaxed), 2, "sent, then sent again");
}

#[tokio::test]
async fn the_bx20_times_out_when_no_answer_comes() {
    // Every answer to a zoom command is lost. After 3 attempts the GCS gets 504 `timeout`,
    // which means "it may or may not have happened": here the commands did arrive, so the
    // zoom moved. That's why the README tells the GCS to re-read after a timeout.
    let commands = Arc::new(AtomicUsize::new(0));
    let counted = commands.clone();
    let setup = start_behind_proxy(Camera::Bx20, move |direction, datagram| {
        let Some(message) = mavlink_message(datagram) else {
            return true;
        };
        if direction == Direction::ToCamera && is_zoom_command(&message) {
            counted.fetch_add(1, Ordering::Relaxed);
        }
        !(direction == Direction::ToPlatform && is_zoom_answer(&message))
    })
    .await;

    let reply = setup.api.set("zoom", json!(7)).await;
    assert_eq!(reply.status, 504, "{}", reply.body);
    assert_eq!(reply.body["error"]["code"], "timeout");
    assert_eq!(commands.load(Ordering::Relaxed), 3, "3 attempts");
    setup.api.wait_for_value("zoom", json!(7.0)).await;
}

#[tokio::test]
async fn the_vlr10_never_resends_a_step() {
    // Going from 1x to 3x takes two single steps. The answer to the first one is lost. A step
    // is never sent twice (the camera might have done it already, and would step twice), so
    // the GCS gets 504 `timeout` after one attempt. The zoom moved part of the way: 2x.
    let commands = Arc::new(AtomicUsize::new(0));
    let counted = commands.clone();
    let mut lost_one = false;
    let setup = start_behind_proxy(Camera::Vlr10, move |direction, datagram| {
        let Some(message) = mavlink_message(datagram) else {
            return true;
        };
        if direction == Direction::ToCamera && is_zoom_command(&message) {
            counted.fetch_add(1, Ordering::Relaxed);
        }
        if direction == Direction::ToPlatform && is_zoom_answer(&message) && !lost_one {
            lost_one = true;
            return false;
        }
        true
    })
    .await;

    let reply = setup.api.set("zoom", json!(3)).await;
    assert_eq!(reply.status, 504, "{}", reply.body);
    assert_eq!(reply.body["error"]["code"], "timeout");
    assert_eq!(
        commands.load(Ordering::Relaxed),
        1,
        "the step isn't sent again"
    );
    setup.api.wait_for_value("zoom", json!(2.0)).await;
}

// ─── Recognising the traffic ─────────────────────────────────────────────────────────────

/// Whether `datagram` is an LC-30T write from the platform. A frame starts with `#tp`, then
/// the source (`P`, the platform), the target, one length character and the control
/// character, which is `w` for a write: `#tpPM4wMUL015508`.
fn is_lc30t_write(datagram: &[u8]) -> bool {
    datagram.get(3) == Some(&b'P') && datagram.get(6) == Some(&b'w')
}

/// The MAVLink message in `datagram`, decoded with the same crate the platform uses.
fn mavlink_message(datagram: &[u8]) -> Option<MavMessage> {
    let mut reader = PeekReader::new(datagram);
    let (_header, message) = read_any_msg::<MavMessage, _>(&mut reader).ok()?;
    Some(message)
}

/// Whether the message is a zoom command, as both MAVLink cameras use: SET_CAMERA_ZOOM.
///
/// Rust: `matches!` checks a value against a pattern, here with an `if` guard, and gives
/// `true` or `false`.
fn is_zoom_command(message: &MavMessage) -> bool {
    matches!(message, MavMessage::COMMAND_LONG(command)
        if command.command == MavCmd::MAV_CMD_SET_CAMERA_ZOOM)
}

/// Whether the message is the camera's answer to a zoom command.
fn is_zoom_answer(message: &MavMessage) -> bool {
    matches!(message, MavMessage::COMMAND_ACK(ack)
        if ack.command == MavCmd::MAV_CMD_SET_CAMERA_ZOOM)
}
