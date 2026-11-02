use std::collections::{BTreeMap};

use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tracing::{info, warn};



use crate::uri::{CameraUri};
use crate::drivers::{self, Driver};
use crate::error::{PayloadError, ConnectError};
use crate::model::{
    ConnectionStatus, Kind, Property, Reading, Snapshot, Timestamp, Value,
};

const QUEUE_LENGTH: usize = 16;
const RECONNECT_PERIOD: u8 = 10;

enum Job {
    Write {
        property: String,
        value: Value,
        answer: oneshot::Sender<Result<Property, PayloadError>>,
    },
    Invoke {
        action: String,
        arguments: BTreeMap<String, Value>,
        answer: oneshot::Sender<Result<BTreeMap<String, Reading>, PayloadError>>,
    },
}

#[derive(Clone)]
pub struct PayloadHandle {
    jobs: mpsc::Sender<Job>,
    snapshot: watch::Receiver<Snapshot>,
}

impl PayloadHandle {

    /// The latest Snapshot
    /// borrow() takes the lock from the channel and gives read access
    /// cloning it right away gives back the lock since we have our own copy now.
    /// So, readers never block the core!
    /// for now cloning a Snapshot is okay, comparing with the cost of an HTTP request
    /// if it become too large we can store is as an Arc<Snapshot>, so cloning is a just
    /// a counter +1
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.borrow().clone()
    }

    // this way the http will hold the lock until we return the answer back
    // in the meantime the core, couldn't be able to update the Snapshot
    // pub fn snapshot(&self) -> watch::Ref<'_, Snapshot> {
    //     self.snapshot.borrow()
    // }

    // pub async fn write(&self, property: &str, value: Value) -> Result<Property, PayloadError> {

    // }

    // pub async fn invoke(
    //     &self,
    //     action: &str,
    // )
}

pub fn start(uri: CameraUri) -> (PayloadHandle, JoinHandle<Result<(), ConnectError>>) {
    // jobs: mpsc::Sender<Job>
    // queue: mpsc::Receiver<Job>
    // we set a limit in case camera hang and GCS keeps sending commands
    let (jobs, queue) = mpsc::channel(QUEUE_LENGTH);
    // a watch channel holds exactly one value, the latest snapshot
    // publisher: watch::Sender<Snapshot>
    // snapshot: watch::Receiver<Snapshot>
    let (publisher, snapshot) = watch::channel(Snapshot::connecting());

    let task = tokio::spawn(run(uri, queue, publisher));
    info!("run() has been called here");
    (PayloadHandle {jobs, snapshot}, task)
}

async fn run(
    uri: CameraUri,
    mut queue: mpsc::Receiver<Job>,
    publisher: watch::Sender<Snapshot>,
) -> Result<(), ConnectError> {
    info!("run() is running");
    let mut driver = connect(uri).await?;
    Ok(());
}

// we are using Box, which is a pointer to the heap
// it makes only sense to use it when we don't know the size of the value
async fn connect(uri: CameraUri) -> Result<Box<dyn Driver>, ConnectError> {
    loop {
        info!("Loop running - trying to connect to camera hardware");
        match drivers::connect(&uri).await {
            Ok(driver) => return Ok(driver),
            Err(ConnectError::NoAnswer(reason)) => {
                warn!("{reason}; no retrying ");
                // warn!("{reason}; retrying in {}s", RECONNECT_PERIOD.as_secs());
            }
            Err(fatal) => return Err(fatal),
        }
    }

}