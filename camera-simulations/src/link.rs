//! The simulators' UDP socket.

use std::{io, net::SocketAddr};

use tokio::net::UdpSocket;
use tracing::warn;

pub struct Link {
    socket: UdpSocket,
}

impl Link {
    pub async fn bind(port: u16) -> io::Result<Self> {
        let socket = UdpSocket::bind(("0.0.0.0", port)).await?;
        Ok(Self { socket })
    }

    #[cfg(test)]
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Receives the next datagram. Cancel safe.
    pub async fn recv(&self, buffer: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.socket.recv_from(buffer).await
    }

    pub async fn send(&self, datagram: Vec<u8>, destination: SocketAddr) {
        if let Err(e) = self.socket.send_to(&datagram, destination).await {
            warn!("failed to send to {destination}: {e}");
        }
    }
}
