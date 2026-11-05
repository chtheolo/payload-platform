use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{MissedTickBehavior, interval, sleep, timeout};
use tracing::{info, warn};

use crate::drivers::{self, Driver};
use crate::error::{ConnectError, PayloadError};
use crate::model::{
    ConnectionStatus, Kind, Property, Reading, Snapshot, Timestamp, Value,
};
use crate::uri::CameraUri;

// README, "Timing and liveness".
const QUEUE_LENGTH: usize = 16;
const REQUEST_DEADLINE: Duration = Duration::from_secs(5);
const POLL_PERIOD: Duration = Duration::from_secs(1);
const OFFLINE_AFTER: Duration = Duration::from_secs(3);
const RECONNECT_PERIOD: Duration = Duration::from_secs(2);


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

    /// Sets a property (`PUT /v1/payload/properties/{id}`).
    pub async fn write(&self, property: &str, value: Value) -> Result<Property, PayloadError> {
        let snapshot = self.snapshot();
        ensure_online(&snapshot)?;
        let current = snapshot.properties.get(property).ok_or_else(|| {
            PayloadError::NotFound(format!("this payload has no property `{property}`"))
        })?;
        if !current.writable {
            return Err(PayloadError::ReadOnly(property.to_string()));
        }
        let value = check(property, &current.kind, value)?;

        // make a pair : sender + receiver
        let (answer, reply) = oneshot::channel(); // 
        self.submit(Job::Write {
            property: property.to_string(),
            value,
            answer,
        })?;
        wait(reply).await
    }


    pub async fn invoke() {
        todo()
    }

    /// Queues a Job. `try_send` never waits, if the queue is full it answers `BUSY` and we send back an Error
    fn submit(&self, job: Job) -> Result<(), PayloadError> {
        // jobs: mpsc::Sender::try_send()
        match self.jobs.try_send(job) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(PayloadError::Busy),
            Err(TrySendError::Closed(_)) => Err(stopped()),
        }
    }
}

/// starts the core for the camera at `uri`
pub fn start(uri: CameraUri) -> (PayloadHandle, JoinHandle<Result<(), ConnectError>>) {
    // jobs: mpsc::Sender<Job>
    // queue: mpsc::Receiver<Job>
    // The job queue: we set a limit in case camera hang and GCS keeps sending commands, created once
    let (jobs, queue) = mpsc::channel(QUEUE_LENGTH);

    // a watch channel holds exactly one value, the latest snapshot
    // publisher: watch::Sender<Snapshot>
    // snapshot: watch::Receiver<Snapshot>
    let (publisher, snapshot) = watch::channel(Snapshot::connecting());

    // the CORE Task starts here for us
    let task = tokio::spawn(run(uri, queue, publisher));
    (PayloadHandle {jobs, snapshot}, task)
}

async fn run(
    uri: CameraUri,
    mut queue: mpsc::Receiver<Job>,
    publisher: watch::Sender<Snapshot>,
) -> Result<(), ConnectError> {
    let mut driver = connect(uri).await?;       // connect to the camera, with retries 

    // publish who the camera is and what it can do
    publisher.send_modify(|snapshot| {
        snapshot.identity = 
    })

    let mut poll = interval(POLL_PERIOD);
    // if a slow command makes us miss ticks, dont fire them all at once afterwards
    poll.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        // select! waits for whichever comes first, a job or the next poll tick.
        tokio::select! {
            job = queue.recv() => match job {
                Some(job) => handle()
            }
        }
    }
    Ok(())
}

// we are using Box, which is a pointer to the heap
// it makes only sense to use it when we don't know the size of the value
async fn connect(uri: CameraUri) -> Result<Box<dyn Driver>, ConnectError> {
    loop {
        info!("trying to connect to camera hardware...");
        match drivers::connect(&uri).await {
            Ok(driver) => return Ok(driver),
            Err(ConnectError::NoAnswer(reason)) => {
                warn!("{reason}; retrying in {}s", RECONNECT_PERIOD.as_secs());
                sleep(RECONNECT_PERIOD).await;
            }
            Err(fatal) => return Err(fatal),
        }
    }
}

/// Validation step: Check if this value is allowed for this property
fn check(name: &str, kind: &Kind, value: Value) -> Result<Value, PayloadError> {
    match (kind, value) {
        (
            Kind::Number {
                minimum,
                maximum,
                resolution,
                ..      // ignore other fields, unit
            },
            Value::Number(number),
        ) => {
            if number < *minimum || number > *maximum {
                return Err(PayloadError::OutOfRange {
                    property: name.to_string(),
                    minimum: *minimum,
                    maximum: *maximum,
                });
            }
            Ok(Value::Number(round_to(number, *resolution)))
        }
        (Kind::Enum { options }, Value::Text(text)) => {
            if options.contains(&text) {
                Ok(Value::Text(text))
            }
            else {
                Err(PayloadError::UnsupportedValue {
                    property: name.to_string(),
                    value: text,
                    options: options.clone(),
                })
            }
        }
        (Kind::Number { .. }, _) => Err(PayloadError::InvalidRequest(format!(
            "`{name}` takes a number"
        ))),
        (Kind::Enum { options }, _) => Err(PayloadError::InvalidRequest(format!(
            "`{name}` takes one of: {}",
            options.join(", ")
        ))),
    }
}

/// Quick check before we send a command. Is the camera online?
fn ensure_online(snapshot: &Snapshot) -> Result<(), PayloadError> {
    if snapshot.connection.status == ConnectionStatus::Online {
        Ok(())
    } else {
        Err(PayloadError::PayloadOffline)
    }
}