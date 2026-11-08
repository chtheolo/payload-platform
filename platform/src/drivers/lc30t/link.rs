//! The UDP connection to one LC-30T.
//!
//! The pod never speaks first: it doesn't answer writes at all, and a read is answered by the
//! module that was read (protocol 3.3). Like the MAVLink link, this keeps two background tasks:
//!
//! - `probe` reads the zoom position once a second. The pod has no heartbeat, so the answers
//!   to these reads are how we notice that it went silent, or came back;
//! - `receive` decodes every datagram from the pod, notes when the pod was last heard, and
//!   publishes each frame on a broadcast channel, where a read finds its answer.
//!
//! Every frame goes out through `Outbox::send`, which keeps the gap the vendor asks for between
//! two frames (protocol 6.2), whichever task sends them.

use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::task::JoinHandle;
use tokio::time::{interval, sleep, timeout};
use tracing::debug;

use super::frame::{self, Frame, HOST, LENS};
use crate::error::DriverError;
use crate::telemetry;

/// How often the probe asks, and what it reads: the zoom position (protocol 4.1). Nothing else
/// reads ZOM, so the probe's answers can't be mistaken for the answer to a real read.
const PROBE_PERIOD: Duration = Duration::from_secs(1);
const PROBE: &str = "ZOM";

/// How long one read waits for its answer, and how many times a read is tried.
const ATTEMPT_TIMEOUT: Duration = Duration::from_millis(500);
const ATTEMPTS: u8 = 3;

pub struct Link {
    outbox: Outbox,
    /// Every frame from the pod is published here; reads subscribe to find their answer.
    incoming: broadcast::Sender<Frame>,
    /// When the pod was last heard from. The `receive` task writes it.
    last_heard: Arc<Mutex<Option<Instant>>>,
    /// The background tasks, stopped when the link is dropped.
    tasks: Vec<JoinHandle<()>>,
}

impl Link {
    /// Opens a UDP socket towards the pod and starts the two background tasks. `gap` is the
    /// shortest time allowed between two frames.
    pub async fn open(address: SocketAddr, gap: Duration) -> io::Result<Link> {
        // The same socket set-up as the MAVLink link: any free local port, "connected" to the
        // pod, so the OS drops datagrams from anyone else. The pod answers to the address and
        // port a frame came from (protocol 2), which is this socket.
        let local = if address.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let socket = UdpSocket::bind(local).await?;
        socket.connect(address).await?;

        let outbox = Outbox {
            socket: Arc::new(socket),
            gap,
            last_sent: Arc::new(tokio::sync::Mutex::new(None)),
        };
        let (incoming, _) = broadcast::channel(64);
        let last_heard = Arc::new(Mutex::new(None));

        let tasks = vec![
            tokio::spawn(probe(outbox.clone())),
            tokio::spawn(receive(
                outbox.socket.clone(),
                incoming.clone(),
                last_heard.clone(),
            )),
        ];
        Ok(Link {
            outbox,
            incoming,
            last_heard,
            tasks,
        })
    }

    /// How long ago anything arrived from the pod; `None` if nothing has arrived yet.
    pub fn last_heard(&self) -> Option<Duration> {
        let last = *self.last_heard.lock().expect("last_heard lock poisoned");
        last.map(|instant| instant.elapsed())
    }

    /// Reads a setting: sends a read to `module` and returns the data of its answer, which
    /// comes from that module with the same identifier (protocol 3.3).
    ///
    /// Reading twice does no harm, so a read that gets no answer is simply sent again.
    pub async fn read(&self, module: char, identifier: &str) -> Result<String, DriverError> {
        let request = Frame::read(module, identifier);
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                telemetry::camera_resend();
            }
            // Subscribe *before* sending, so a quick answer can't arrive while nobody listens.
            let mut frames = self.incoming.subscribe();
            self.outbox.send(&request).await;
            let answer = async {
                while let Some(frame) = next(&mut frames).await {
                    if frame.source == module && frame.identifier == identifier {
                        return Some(frame.data);
                    }
                }
                None
            };
            // Rust: `Ok(Some(data))` matches only "in time, and with an answer".
            if let Ok(Some(data)) = timeout(ATTEMPT_TIMEOUT, answer).await {
                return Ok(data);
            }
        }
        telemetry::camera_unanswered();
        Err(DriverError::Timeout)
    }

    /// Sends a write. The pod never answers one, so the driver confirms it with a read.
    pub async fn write(&self, module: char, identifier: &str, data: &str) {
        self.outbox
            .send(&Frame::write(module, identifier, data))
            .await;
    }
}

/// Stops the background tasks, so a link that is thrown away also stops probing.
impl Drop for Link {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// The sending side of the link, shared with the probe task.
#[derive(Clone)]
struct Outbox {
    socket: Arc<UdpSocket>,
    /// The shortest time allowed between two frames.
    gap: Duration,
    /// When the last frame went out.
    ///
    /// Rust: this is tokio's `Mutex`, not std's. `send` keeps it locked while it waits and
    /// sends, and only tokio's may stay locked across an `.await`. While one task holds the
    /// lock, any other task that wants to send waits its turn, so the gap holds for everyone.
    last_sent: Arc<tokio::sync::Mutex<Option<Instant>>>,
}

impl Outbox {
    /// Sends one frame, first waiting until `gap` has passed since the previous frame.
    /// Failures are only logged, as in the MAVLink link: every read has its own timeout.
    async fn send(&self, frame: &Frame) {
        let mut last_sent = self.last_sent.lock().await;
        if let Some(last) = *last_sent {
            // Rust: `checked_duration_since` gives `None` when that moment has already passed.
            if let Some(wait) = (last + self.gap).checked_duration_since(Instant::now()) {
                sleep(wait).await;
            }
        }
        let text = frame.encode();
        if let Err(error) = self.socket.send(text.as_bytes()).await {
            // "Connection refused" here usually means the pod isn't running.
            debug!("sending {text} failed: {error}");
        }
        *last_sent = Some(Instant::now());
    }
}

/// Background task: the liveness probe, once a second for as long as the link exists.
async fn probe(outbox: Outbox) {
    let request = Frame::read(LENS, PROBE);
    let mut ticks = interval(PROBE_PERIOD);
    loop {
        ticks.tick().await;
        outbox.send(&request).await;
    }
}

/// Background task: receives datagrams, keeps the frames addressed to us, notes when the pod
/// was last heard, and publishes each frame for the reads to pick from.
async fn receive(
    socket: Arc<UdpSocket>,
    incoming: broadcast::Sender<Frame>,
    last_heard: Arc<Mutex<Option<Instant>>>,
) {
    let mut buffer = [0u8; 2048];
    loop {
        let length = match socket.recv(&mut buffer).await {
            Ok(length) => length,
            Err(error) => {
                // As in the MAVLink link: "connection refused" while nothing listens at the
                // pod's address. The pod may start later, so keep going after a short pause.
                debug!("receiving failed: {error}");
                sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        for decoded in frame::decode(&buffer[..length]) {
            match decoded {
                // Answers go to the host that asked (protocol 3.3), and that's us.
                Ok(frame) if frame.target == HOST => {
                    *last_heard.lock().expect("last_heard lock poisoned") = Some(Instant::now());
                    // Rust: `send` fails only when nobody is subscribed, which is normal.
                    let _ = incoming.send(frame);
                }
                Ok(frame) => debug!("ignoring a frame for `{}`", frame.target),
                Err(error) => debug!("dropping a frame: {error}"),
            }
        }
    }
}

/// The next frame from the pod, or `None` once the link is gone. The same as in the MAVLink
/// link: falling behind (`Lagged`) only means "keep reading".
async fn next(frames: &mut broadcast::Receiver<Frame>) -> Option<Frame> {
    loop {
        match frames.recv().await {
            Ok(frame) => return Some(frame),
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => return None,
        }
    }
}
