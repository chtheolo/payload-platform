//! What the tests in this folder share: starting the real simulators and the real platform as
//! processes, and calling the platform's HTTP API the way a GCS would.
//!
//! Rust: each file directly inside `tests/` is compiled as a crate of its own, which sees the
//! platform only from the outside, through its binary. `tests/common/` is a module rather than
//! a test file, and each test file pulls it in with `mod common;`.

// Rust: every test file compiles its own copy of this module and uses only part of it. Without
// this line, the parts a file doesn't use would each raise an "unused" warning.
#![allow(dead_code)]

pub mod proxy;

use std::fs::File;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::{Mutex, MutexGuard};
use tokio::time::sleep;

use self::proxy::{Direction, Proxy};

/// How long to wait for something the platform should do on its own, such as coming online.
/// Generous: a MAVLink camera that starts late can take about 5 s (3 s waiting for its first
/// heartbeat, then 2 s before the next try).
const WAIT_LIMIT: Duration = Duration::from_secs(15);

/// The three simulated cameras.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Camera {
    Bx20,
    Vlr10,
    Lc30t,
}

impl Camera {
    /// The simulator's subcommand: `camera-sim bx20`.
    pub fn name(self) -> &'static str {
        match self {
            Camera::Bx20 => "bx20",
            Camera::Vlr10 => "vlr10",
            Camera::Lc30t => "lc30t",
        }
    }

    /// The simulator's UDP port, which can't be changed (camera-simulations/README.md).
    pub fn port(self) -> u16 {
        match self {
            Camera::Bx20 => 14580,
            Camera::Vlr10 => 14581,
            Camera::Lc30t => 9003,
        }
    }

    /// The camera URI for this camera at `port`: its own port, or a proxy's.
    pub fn uri(self, port: u16) -> String {
        let scheme = match self {
            Camera::Bx20 | Camera::Vlr10 => "mavlink",
            Camera::Lc30t => "lc30t",
        };
        format!("{scheme}://127.0.0.1:{port}")
    }
}

/// A running process. It's stopped as soon as this value is dropped, so a failing test never
/// leaves a simulator or a platform running.
///
/// Rust: `Drop` is Rust's destructor. It runs when the value goes out of scope, and also while
/// a failing test unwinds.
pub struct Process {
    child: Child,
    /// The file that collects the process's output. Worth opening when a test fails.
    pub log: PathBuf,
}

impl Process {
    fn start(program: &Path, args: &[&str], name: &str) -> Process {
        // Every process gets its own log file, numbered in start order.
        // Rust: `env!` reads a variable when this file is compiled. For integration tests,
        // Cargo sets CARGO_TARGET_TMPDIR to a scratch folder inside `target/`, and
        // CARGO_CRATE_NAME to the test file's name, which keeps different test files apart.
        static STARTED: AtomicUsize = AtomicUsize::new(1);
        let number = STARTED.fetch_add(1, Ordering::Relaxed);
        let log = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("{}-{number}-{name}.log", env!("CARGO_CRATE_NAME")));
        let file = File::create(&log).expect("can't create the log file");
        let child = Command::new(program)
            .args(args)
            // Plain text logs, without colour codes.
            .env("NO_COLOR", "1")
            .stdout(file.try_clone().expect("can't share the log file"))
            .stderr(file)
            .spawn()
            .unwrap_or_else(|error| panic!("can't start {}: {error}", program.display()));
        Process { child, log }
    }

    /// Stops the process and waits until it's gone, so its port is free again.
    pub fn stop(&mut self) {
        // Rust: both fail only if the process has already ended, which is fine here.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// How the process ended, or `None` while it's still running.
    pub fn exit_status(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// Everything the process has written so far.
    pub fn output(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The simulators' program, `camera-sim`. The first test that needs it builds it, if
/// nobody has yet.
///
/// Rust: `OnceLock` runs the code inside `get_or_init` only once, even when several tests
/// ask at the same moment; the others wait for it and then get the same value.
fn simulator_program() -> &'static Path {
    static PROGRAM: OnceLock<PathBuf> = OnceLock::new();
    PROGRAM.get_or_init(|| {
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../camera-simulations");
        let program = folder.join("target/release/camera-sim");
        if !program.exists() {
            let built = Command::new(env!("CARGO"))
                .args(["build", "--release", "--manifest-path"])
                .arg(folder.join("Cargo.toml"))
                .status()
                .expect("can't run cargo");
            assert!(built.success(), "building the simulators failed");
        }
        program
    })
}

/// Starts the simulator of `camera` on its own port.
pub async fn simulator(camera: Camera) -> Process {
    let name = format!("{}-simulator", camera.name());
    let mut process = Process::start(simulator_program(), &[camera.name()], &name);
    // A simulator that can't get its port, because another copy already has it, stops at once.
    sleep(Duration::from_millis(300)).await;
    if let Some(status) = process.exit_status() {
        panic!(
            "the {} simulator stopped at once ({status}). Is another one running? See {}",
            camera.name(),
            process.log.display()
        );
    }
    process
}

/// Starts the platform for the camera at `uri`, with its API on a free local port.
pub fn platform(uri: &str) -> (Process, Api) {
    let port = free_port();
    let listen = format!("127.0.0.1:{port}");
    // Rust: for integration tests Cargo builds the package's binary first, and passes its
    // path in CARGO_BIN_EXE_<name>; ours is called `platform`.
    let program = Path::new(env!("CARGO_BIN_EXE_platform"));
    let process = Process::start(program, &["--camera", uri, "--listen", &listen], "platform");
    (process, Api::new(port))
}

/// A TCP port nobody is using: binding port 0 lets the system pick one, and dropping the
/// listener right away frees it for the platform.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("no free port");
    listener.local_addr().expect("no local address").port()
}

/// Each simulator has a fixed port, so only one test at a time may run a given camera. A test
/// holds its camera's turn until it ends, while tests of other cameras run alongside.
///
/// Rust: `LazyLock` builds the value the first time it's used. tokio's `Mutex`, unlike the
/// standard library's, may stay locked across an `.await`.
static TURNS: LazyLock<[Mutex<()>; 3]> =
    LazyLock::new(|| [Mutex::new(()), Mutex::new(()), Mutex::new(())]);

/// Waits for `camera`'s turn. The turn ends when the returned guard is dropped.
pub async fn turn(camera: Camera) -> MutexGuard<'static, ()> {
    let index = match camera {
        Camera::Bx20 => 0,
        Camera::Vlr10 => 1,
        Camera::Lc30t => 2,
    };
    TURNS[index].lock().await
}

/// A simulator and a platform talking to it, with the platform online.
///
/// Rust: fields are dropped in the order they're declared. So the platform stops first, then
/// the proxy and the simulator, and the turn is given back last, once all of them are gone.
pub struct Setup {
    pub api: Api,
    pub platform: Process,
    pub proxy: Option<Proxy>,
    pub simulator: Process,
    _turn: MutexGuard<'static, ()>,
}

/// Starts `camera`, then a platform for it, and waits until the platform is online.
pub async fn start(camera: Camera) -> Setup {
    let turn = turn(camera).await;
    let simulator = simulator(camera).await;
    let (platform, api) = platform(&camera.uri(camera.port()));
    api.wait_for("online").await;
    Setup {
        api,
        platform,
        proxy: None,
        simulator,
        _turn: turn,
    }
}

/// Like `start`, with a proxy between the platform and the camera that lets a datagram
/// through only when `passes` says so.
pub async fn start_behind_proxy(
    camera: Camera,
    passes: impl FnMut(Direction, &[u8]) -> bool + Send + 'static,
) -> Setup {
    let turn = turn(camera).await;
    let simulator = simulator(camera).await;
    let proxy = Proxy::start(camera.port(), passes).await;
    let (platform, api) = platform(&camera.uri(proxy.port()));
    api.wait_for("online").await;
    Setup {
        api,
        platform,
        proxy: Some(proxy),
        simulator,
        _turn: turn,
    }
}

/// The platform's HTTP API, seen from the GCS.
///
/// Rust: `reqwest::Client` is cheap to clone (the clones share one connection pool), so the
/// whole `Api` can be cloned too, for example to send requests from several tasks at once.
#[derive(Clone)]
pub struct Api {
    base: String,
    http: reqwest::Client,
}

/// An answer from the API: its HTTP status and its JSON body (`null` if it has none).
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub body: Value,
}

impl Api {
    fn new(port: u16) -> Api {
        let http = reqwest::Client::builder()
            // The platform answers every request within 5 s (README, "GCS API").
            .timeout(Duration::from_secs(10))
            .build()
            .expect("can't build the HTTP client");
        Api {
            base: format!("http://127.0.0.1:{port}"),
            http,
        }
    }

    pub async fn get(&self, path: &str) -> Reply {
        self.send(self.http.get(self.url(path))).await
    }

    pub async fn put(&self, path: &str, body: Value) -> Reply {
        self.send(self.http.put(self.url(path)).json(&body)).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Reply {
        self.send(self.http.post(self.url(path)).json(&body)).await
    }

    /// `GET /v1/payload`: identity, connection, properties and actions.
    pub async fn payload(&self) -> Value {
        self.get("/v1/payload").await.body
    }

    /// Sets a property: `PUT /v1/payload/properties/{property}` with `{"value": ...}`.
    pub async fn set(&self, property: &str, value: Value) -> Reply {
        let path = format!("/v1/payload/properties/{property}");
        self.put(&path, json!({ "value": value })).await
    }

    /// Steps the zoom: `POST /v1/payload/actions/zoom_step` with `{"direction": ...}`.
    pub async fn step(&self, direction: &str) -> Reply {
        let arguments = json!({ "direction": direction });
        self.post("/v1/payload/actions/zoom_step", arguments).await
    }

    /// A property's value as the platform shows it now, from its cache.
    pub async fn value(&self, property: &str) -> Value {
        let path = format!("/v1/payload/properties/{property}");
        self.get(&path).await.body["value"].clone()
    }

    /// Waits until `connection.status` is `status`, and returns how long that took.
    pub async fn wait_for(&self, status: &str) -> Duration {
        let started = Instant::now();
        while started.elapsed() < WAIT_LIMIT {
            if self.status().await.as_deref() == Some(status) {
                return started.elapsed();
            }
            sleep(Duration::from_millis(100)).await;
        }
        panic!(
            "the platform at {} wasn't `{status}` after {WAIT_LIMIT:?}",
            self.base
        );
    }

    /// Waits until `property` shows `value`, for changes the tests don't make themselves,
    /// such as a camera that restarted. Returns how long that took.
    pub async fn wait_for_value(&self, property: &str, value: Value) -> Duration {
        let started = Instant::now();
        while started.elapsed() < WAIT_LIMIT {
            if self.value(property).await == value {
                return started.elapsed();
            }
            sleep(Duration::from_millis(100)).await;
        }
        panic!("`{property}` didn't become {value} within {WAIT_LIMIT:?}");
    }

    /// `connection.status`, or `None` while the platform doesn't answer yet (it may still be
    /// starting up).
    ///
    /// Rust: `?` works on `Option` too: on `None` it returns `None` straight away.
    async fn status(&self) -> Option<String> {
        let response = self.http.get(self.url("/v1/payload")).send().await.ok()?;
        let payload: Value = response.json().await.ok()?;
        payload["connection"]["status"].as_str().map(String::from)
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Reply {
        let response = request.send().await.expect("the platform didn't answer");
        let status = response.status().as_u16();
        // A body that isn't JSON becomes `null` rather than failing the test here; the test
        // then fails on the field it expected, which says more.
        let body = response.json().await.unwrap_or(Value::Null);
        Reply { status, body }
    }
}
