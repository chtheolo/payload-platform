//! Camera pod speaking an ASCII command protocol over plain UDP.

mod fields;
mod frame;
mod pod;

use std::net::SocketAddr;

use tracing::{debug, info, warn};

use crate::{error::SimError, link::Link};
use pod::Pod;

pub async fn run(port: u16) -> Result<(), SimError> {
    let link = Link::bind(port)
        .await
        .map_err(|source| SimError::Bind { port, source })?;
    info!("listening for the UDP camera protocol on port {port}");

    let mut pod = Pod::new();
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    let mut buffer = [0u8; 2048];

    loop {
        tokio::select! {
            _ = &mut shutdown => return Ok(()),
            received = link.recv(&mut buffer) => match received {
                Ok((length, sender)) => {
                    let datagram = buffer.get(..length).unwrap_or_default();
                    handle_datagram(&mut pod, &link, datagram, sender).await;
                }
                Err(e) => warn!("failed to receive: {e}"),
            },
        }
    }
}

async fn handle_datagram(pod: &mut Pod, link: &Link, datagram: &[u8], sender: SocketAddr) {
    for parsed in frame::parse_datagram(datagram) {
        let frame = match parsed {
            Ok(frame) => frame,
            Err(e) => {
                warn!("dropping frame from {sender}: {e}");
                continue;
            }
        };
        info!("{} {:?} from {sender}", frame.identifier(), frame.control);
        let Some(reply) = pod.handle(&frame) else {
            continue;
        };
        let bytes = reply.encode();
        debug!("sending {} to {sender}", String::from_utf8_lossy(&bytes));
        link.send(bytes, sender).await;
    }
}
