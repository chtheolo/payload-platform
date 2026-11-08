//! A UDP proxy that sits between the platform and a simulator and loses chosen datagrams on
//! purpose, to show how the platform copes with a bad link:
//!
//! ```text
//! platform ──► front ─ proxy ─ back ──► simulator
//! platform ◄── front ─ proxy ─ back ◄── simulator
//! ```
//!
//! The platform is given the proxy's port instead of the camera's. Each datagram, in either
//! direction, is shown to a rule, which says whether it gets through.

use std::net::SocketAddr;

use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

/// Which way a datagram is going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    ToCamera,
    ToPlatform,
}

pub struct Proxy {
    port: u16,
    task: JoinHandle<()>,
}

impl Proxy {
    /// Starts a proxy for the simulator on `camera_port`. `passes` sees every datagram and
    /// returns `true` to let it through, or `false` to lose it.
    ///
    /// Rust: `impl FnMut(...) -> bool` accepts any closure with that shape. `FnMut` lets the
    /// closure change what it captured, such as a count of the datagrams it has seen. `Send +
    /// 'static` is what `tokio::spawn` needs to run it in a task of its own.
    pub async fn start(
        camera_port: u16,
        mut passes: impl FnMut(Direction, &[u8]) -> bool + Send + 'static,
    ) -> Proxy {
        // The side the platform talks to, on a port the system picks.
        let front = UdpSocket::bind("127.0.0.1:0").await.expect("can't bind");
        // The side that talks to the simulator.
        let back = UdpSocket::bind("127.0.0.1:0").await.expect("can't bind");
        back.connect(("127.0.0.1", camera_port))
            .await
            .expect("can't reach the simulator's port");
        let port = front.local_addr().expect("no local address").port();

        let task = tokio::spawn(async move {
            // Where the platform's datagrams come from, so that answers can go back there.
            let mut platform: Option<SocketAddr> = None;
            // One buffer per direction: `select!` below waits on both at once, and Rust won't
            // lend the same buffer to two places at the same time.
            let mut to_camera = [0u8; 2048];
            let mut to_platform = [0u8; 2048];
            loop {
                tokio::select! {
                    received = front.recv_from(&mut to_camera) => {
                        let Ok((length, sender)) = received else { continue };
                        platform = Some(sender);
                        let datagram = &to_camera[..length];
                        if passes(Direction::ToCamera, datagram) {
                            let _ = back.send(datagram).await;
                        }
                    }
                    // While the simulator is stopped this fails ("connection refused"); the
                    // proxy just keeps going.
                    received = back.recv(&mut to_platform) => {
                        let Ok(length) = received else { continue };
                        let datagram = &to_platform[..length];
                        // Rust: `if let ... && ...` matches a pattern and checks a condition
                        // in one `if`. Answers wait until the platform has sent something.
                        if let Some(platform) = platform
                            && passes(Direction::ToPlatform, datagram)
                        {
                            let _ = front.send_to(datagram, platform).await;
                        }
                    }
                }
            }
        });
        Proxy { port, task }
    }

    /// The port to give the platform instead of the camera's.
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        // The task would otherwise go on forever; aborting it also closes both sockets.
        self.task.abort();
    }
}
