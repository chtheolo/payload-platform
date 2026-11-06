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
    ConnectionStatus, Kind, Property, Reading, Snapshot, Timestamp, Value, round_to,
};
use crate::uri::CameraUri;

// README, "Timing and liveness".
const QUEUE_LENGTH: usize = 16;
const REQUEST_DEADLINE: Duration = Duration::from_secs(5);
const POLL_PERIOD: Duration = Duration::from_secs(1);
const OFFLINE_AFTER: Duration = Duration::from_secs(3);
const RECONNECT_PERIOD: Duration = Duration::from_secs(2);

// two kind of Jobs:
// 1. "set this property to this value" 
// PUT /v1/payload/properties/{id} : PUT /v1/payload/properties/zoom {"value": 10.5}
// 2. "do this action with these arguments
// POST /v1/payload/actions/{id} : POST /v1/payload/actions/zoom_step {"direction": "in"}
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
        let (answer, reply) = oneshot::channel();
        self.submit(Job::Write {
            property: property.to_string(),
            value,
            answer,
        })?;
        wait(reply).await // wait for the reply
    }

    /// Runs an action (`POST /v1/payload/actions/{id}`).
    pub async fn invoke(
        &self,
        action: &str,
        arguments: BTreeMap<String, Value>,
    ) -> Result<BTreeMap<String, Reading>, PayloadError> {
        let snapshot = self.snapshot();
        ensure_online(&snapshot)?;
        let described = snapshot.actions.get(action).ok_or_else(|| {
            PayloadError::NotFound(format!("this payload has no action `{action}`"))
        })?;
        // Every argument must be one the action takes...
        if let Some(unknown) = arguments
            .keys()
            .find(|name| !described.parameters.contains_key(*name))
        {
            return Err(PayloadError::InvalidRequest(format!(
                "`{action}` has no argument `{unknown}`"
            )));
        }
        // ...and every parameter must be given, with a valid value.
        let mut checked = BTreeMap::new();
        for (name, kind) in &described.parameters {
            let value = arguments.get(name).cloned().ok_or_else(|| {
                PayloadError::InvalidRequest(format!("`{action}` needs the argument `{name}`"))
            })?;
            checked.insert(name.clone(), check(name, kind, value)?);
        }

        let (answer, reply) = oneshot::channel();
        self.submit(Job::Invoke {
            action: action.to_string(),
            arguments: checked,
            answer,
        })?;
        wait(reply).await
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

    // Publish who the camera is and what it can do. The values follow with the first poll.
    publisher.send_modify(|snapshot| {
        snapshot.identity = Some(driver.identity());
        snapshot.properties = driver.properties();
        snapshot.actions = driver.actions();
    });

    let mut poll = interval(POLL_PERIOD);
    // if a slow command makes us miss ticks, dont fire them all at once afterwards
    poll.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        // select! waits for whichever comes first, a job or the next poll tick.
        tokio::select! {
            job = queue.recv() => match job {
                Some(job) => handle(job, driver.as_mut(), &publisher).await,
                // Every handle is gone: the platform is shutting down.
                None => return Ok(()),
            },
            _ = poll.tick() => refresh(driver.as_mut(), &publisher).await,
        }
    }
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

/// Runs one job on the driver and sends back the answer.
async fn handle(job: Job, driver: &mut dyn Driver, publisher: &watch::Sender::<Snapshot>) {
    match job {
        Job::Write {
            property,
            value,
            answer,
        } => {
            let result = match driver.write(&property, &value).await {
                Ok(observed) => {
                    store(publisher, BTreeMap::from([(property.clone(), observed)]));
                    // Answer with the whole property, the way `GET` would now show it.
                    publisher
                        .borrow()
                        .properties
                        .get(&property)
                        .cloned()
                        .ok_or_else(|| PayloadError::Internal(format!("`{property}` vanished")))
                }
                Err(error) => Err(error.into()),
            };
            // Rust: `send` fails if the HTTP request already gave up (its deadline passed).
            // There's nothing left to do then, so the result is ignored on purpose.
            let _ = answer.send(result);
        }
        Job::Invoke {
            action,
            arguments,
            answer,
        } => {
            let result = match driver.invoke(&action, &arguments).await {
                Ok(changed) => Ok(store(publisher, changed)),
                Err(error) => Err(error.into()),
            };
            let _ = answer.send(result);
        }
    }
}

/// Updates the connection status, then re-reads every property while the camera is online.
async fn refresh(driver: &mut dyn Driver, publisher: &watch::Sender<Snapshot>) {
    let heard = driver.last_heard();
    // One rule for every driver (README, "Timing and liveness").
    let status = match heard {
        Some(age) if age < OFFLINE_AFTER => ConnectionStatus::Online,
        _ => ConnectionStatus::Offline,
    };
    let previous = publisher.borrow().connection.status;
    if status != previous {
        info!("camera is now {status:?}");
    }
    publisher.send_modify(|snapshot| {
        snapshot.connection.status = status;
        // Turn "heard 0.4 s ago" into a clock time the GCS can show.
        snapshot.connection.last_seen = heard
            .and_then(|age| SystemTime::now().checked_sub(age))
            .map(Timestamp);
    });

    if status != ConnectionStatus::Online {
        return; // asking a silent camera would only stack up timeouts
    }
    match driver.read().await {
        Ok(values) => {
            store(publisher, values);
        }
        Err(error) => warn!("reading the camera failed: {error}"),
    }
}

fn store(
    publisher: &watch::Sender<Snapshot>,
    values: BTreeMap<String, Value>,
) -> BTreeMap<String, Reading> {
    let now = Timestamp::now();
    publisher.send_modify(|snapshot| {
        for (name, value) in &values {
            if let Some(property) = snapshot.properties.get_mut(name) {
                property.value = Some(value.clone());
                property.observed_at = Some(now);
            }
        }
    });
    values
        .into_iter()
        .map(|(name, value)| (name, Reading {value, observed_at: now}))
        .collect()
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

/// Waits for the core's answer, but never longer than the 5-second deadline.
async fn wait<T>(reply: oneshot::Receiver<Result<T, PayloadError>>) -> Result<T, PayloadError> {
    match timeout(REQUEST_DEADLINE, reply).await {
        Ok(Ok(result)) => result,
        // The core dropped the job without answering: it has stopped.
        Ok(Err(_)) => Err(stopped()),
        Err(_) => Err(PayloadError::Timeout),
    }
}

fn stopped() -> PayloadError {
    PayloadError::Internal("the payload core has stopped".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zoom() -> Kind {
        Kind::Number {
            unit: "x".into(),
            minimum: 1.0,
            maximum: 20.0,
            resolution: 0.1,
        }
    }

    #[test]
    fn numbers_are_rounded_to_the_resolution() {
        let checked = check("zoom", &zoom(), Value::Number(4.04)).unwrap();
        assert_eq!(checked, Value::Number(4.0));
    }

    #[test]
    fn numbers_out_of_range_are_refused() {
        let checked = check("zoom", &zoom(), Value::Number(25.0));
        assert!(matches!(checked, Err(PayloadError::OutOfRange { .. })));
    }

    #[test]
    fn enum_values_must_be_offered() {
        let palette = Kind::Enum {
            options: vec!["white_hot".into(), "ironbow".into()],
        };
        assert!(check("thermal_palette", &palette, Value::Text("ironbow".into())).is_ok());
        let lava = check("thermal_palette", &palette, Value::Text("lava".into()));
        assert!(matches!(lava, Err(PayloadError::UnsupportedValue { .. })));
    }

    #[test]
    fn a_wrong_type_is_an_invalid_request() {
        let checked = check("zoom", &zoom(), Value::Text("ten".into()));
        assert!(matches!(checked, Err(PayloadError::InvalidRequest(_))));
    }
}
