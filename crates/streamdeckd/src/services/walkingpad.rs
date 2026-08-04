use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use streamdeck_core::deadline::Backoff;
use streamdeck_core::integrations::walkingpad::{
    distance_hundredths, speed_tenths, WalkingPadConnection, WalkingPadCounters, WalkingPadMode,
    WalkingPadRequest, WalkingPadTelemetry, WalkingPadUpdate, STATUS_STALE_AFTER_MS,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use walkingpad::{CommandLock, DeviceStore, Mode, SavedDevice, Speed, WalkingPad};

use crate::runtime::RuntimeEvent;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(8);
const STATUS_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(900);
const RECONNECT_BASE_MS: u64 = 1_000;
const RECONNECT_MAX_MS: u64 = 60_000;

pub trait WalkingPadCommander: Send + Sync {
    fn send(&self, request: WalkingPadRequest) -> Result<(), String>;
}

#[derive(Clone)]
pub struct WalkingPadController {
    urgent: mpsc::UnboundedSender<UrgentRequest>,
    normal: mpsc::UnboundedSender<WalkingPadRequest>,
}

impl WalkingPadCommander for WalkingPadController {
    fn send(&self, request: WalkingPadRequest) -> Result<(), String> {
        match request {
            WalkingPadRequest::Stop => self
                .urgent
                .send(UrgentRequest::Stop)
                .map_err(|_| "WalkingPad service is not running".to_string()),
            request => self
                .normal
                .send(request)
                .map_err(|_| "WalkingPad service is not running".to_string()),
        }
    }
}

pub struct WalkingPadService {
    controller: Arc<WalkingPadController>,
    task: JoinHandle<()>,
}

impl WalkingPadService {
    pub fn spawn(events: mpsc::UnboundedSender<RuntimeEvent>) -> Result<Self, String> {
        let backend = RealBackend::new()?;
        Ok(Self::spawn_with_backend(events, Box::new(backend)))
    }

    fn spawn_with_backend(
        events: mpsc::UnboundedSender<RuntimeEvent>,
        backend: Box<dyn PadBackend>,
    ) -> Self {
        let (urgent, urgent_rx) = mpsc::unbounded_channel();
        let (normal, normal_rx) = mpsc::unbounded_channel();
        let controller = Arc::new(WalkingPadController { urgent, normal });
        let task = tokio::spawn(run(backend, urgent_rx, normal_rx, events));
        Self { controller, task }
    }

    pub fn controller(&self) -> Arc<dyn WalkingPadCommander> {
        Arc::clone(&self.controller) as Arc<dyn WalkingPadCommander>
    }

    pub async fn shutdown(self) {
        let _ = self.controller.urgent.send(UrgentRequest::Shutdown);
        let mut task = self.task;
        if tokio::time::timeout(Duration::from_secs(4), &mut task)
            .await
            .is_err()
        {
            tracing::warn!(
                component = "walkingpad",
                "WalkingPad service did not stop within its grace period"
            );
            task.abort();
            let _ = task.await;
        }
    }
}

pub struct UnavailableWalkingPadController {
    reason: String,
}

impl UnavailableWalkingPadController {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl WalkingPadCommander for UnavailableWalkingPadController {
    fn send(&self, _request: WalkingPadRequest) -> Result<(), String> {
        Err(self.reason.clone())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UrgentRequest {
    Stop,
    Shutdown,
}

#[derive(Debug)]
struct ConnectFailure {
    locked: bool,
    message: String,
}

impl ConnectFailure {
    fn unavailable(error: impl std::fmt::Display) -> Self {
        Self {
            locked: false,
            message: error.to_string(),
        }
    }

    fn locked(error: impl std::fmt::Display) -> Self {
        Self {
            locked: true,
            message: error.to_string(),
        }
    }
}

#[async_trait]
trait PadBackend: Send {
    async fn connect(&mut self) -> Result<Box<dyn PadConnection>, ConnectFailure>;
}

#[async_trait]
trait PadConnection: Send {
    async fn start(&mut self) -> Result<(), String>;
    async fn halt(&mut self) -> Result<(), String>;
    async fn set_speed(&mut self, tenths: u8) -> Result<(), String>;
    async fn status(&mut self, timeout: Duration) -> Result<WalkingPadTelemetry, String>;
    async fn disconnect(self: Box<Self>) -> Result<(), String>;
}

struct RealBackend {
    store: DeviceStore,
    lock: Option<CommandLock>,
}

impl RealBackend {
    fn new() -> Result<Self, String> {
        Ok(Self {
            store: DeviceStore::discover().map_err(|error| error.to_string())?,
            lock: None,
        })
    }

    fn acquire_lock(&mut self) -> Result<(), ConnectFailure> {
        if self.lock.is_some() {
            return Ok(());
        }
        match self.store.try_lock() {
            Ok(lock) => {
                self.lock = Some(lock);
                Ok(())
            }
            Err(walkingpad::Error::CommandInProgress) => {
                Err(ConnectFailure::locked("controlled by another process"))
            }
            Err(error) => Err(ConnectFailure::unavailable(error)),
        }
    }
}

#[async_trait]
impl PadBackend for RealBackend {
    async fn connect(&mut self) -> Result<Box<dyn PadConnection>, ConnectFailure> {
        self.acquire_lock()?;

        if let Some(saved) = self.store.load().map_err(ConnectFailure::unavailable)? {
            match WalkingPad::open(&saved.id, CONNECT_TIMEOUT).await {
                Ok(pad) => {
                    tracing::debug!(
                        component = "walkingpad",
                        device_id = %saved.id,
                        "opened saved WalkingPad without scanning"
                    );
                    return Ok(Box::new(RealConnection { pad }));
                }
                Err(error) => tracing::debug!(
                    component = "walkingpad",
                    device_id = %saved.id,
                    error = %error,
                    "saved WalkingPad could not be opened; scanning"
                ),
            }
        }

        let discovered = WalkingPad::discover(DISCOVERY_TIMEOUT)
            .await
            .map_err(ConnectFailure::unavailable)?;
        let saved = SavedDevice {
            id: discovered.id(),
            name: discovered.name().to_owned(),
        };
        let pad = discovered
            .connect(CONNECT_TIMEOUT)
            .await
            .map_err(ConnectFailure::unavailable)?;
        if let Err(error) = self.store.save(&saved) {
            tracing::warn!(
                component = "walkingpad",
                error = %error,
                "connected but could not persist the WalkingPad identifier"
            );
        }
        Ok(Box::new(RealConnection { pad }))
    }
}

struct RealConnection {
    pad: WalkingPad,
}

#[async_trait]
impl PadConnection for RealConnection {
    async fn start(&mut self) -> Result<(), String> {
        self.pad.start().await.map_err(|error| error.to_string())
    }

    async fn halt(&mut self) -> Result<(), String> {
        self.pad.halt().await.map_err(|error| error.to_string())
    }

    async fn set_speed(&mut self, tenths: u8) -> Result<(), String> {
        let speed = Speed::from_tenths(tenths).map_err(|error| error.to_string())?;
        self.pad
            .set_speed(speed)
            .await
            .map_err(|error| error.to_string())
    }

    async fn status(&mut self, timeout: Duration) -> Result<WalkingPadTelemetry, String> {
        self.pad
            .status(timeout)
            .await
            .map(|status| WalkingPadTelemetry {
                counters: WalkingPadCounters {
                    distance_hundredths: distance_hundredths(status.distance_km),
                    steps: status.steps,
                    elapsed_seconds: status.elapsed_seconds,
                },
                speed_tenths: speed_tenths(status.speed_kph),
                target_speed_tenths: speed_tenths(status.app_speed_kph),
                belt_state: status.belt_state,
                mode: match status.mode {
                    Mode::Automatic => WalkingPadMode::Automatic,
                    Mode::Manual => WalkingPadMode::Manual,
                    Mode::Standby => WalkingPadMode::Standby,
                },
            })
            .map_err(|error| error.to_string())
    }

    async fn disconnect(self: Box<Self>) -> Result<(), String> {
        self.pad
            .disconnect()
            .await
            .map_err(|error| error.to_string())
    }
}

enum ConnectedExit {
    Reconnect(String),
    Shutdown,
}

enum ConnectOutcome {
    Connected(Box<dyn PadConnection>),
    Failed(ConnectFailure),
    Interrupted,
    Shutdown,
}

async fn run(
    mut backend: Box<dyn PadBackend>,
    mut urgent: mpsc::UnboundedReceiver<UrgentRequest>,
    mut normal: mpsc::UnboundedReceiver<WalkingPadRequest>,
    events: mpsc::UnboundedSender<RuntimeEvent>,
) {
    let mut backoff = Backoff::new(RECONNECT_BASE_MS, RECONNECT_MAX_MS);
    let mut last_logged_error = None;

    loop {
        publish_connection(&events, WalkingPadConnection::Connecting, None);
        let mut pad =
            match connect_preemptible(&mut *backend, &mut urgent, &mut normal, &events).await {
                ConnectOutcome::Connected(pad) => {
                    publish_connection(&events, WalkingPadConnection::Connected, None);
                    tracing::debug!(component = "walkingpad", "WalkingPad BLE connection opened");
                    pad
                }
                ConnectOutcome::Failed(error) => {
                    let state = if error.locked {
                        WalkingPadConnection::Locked
                    } else {
                        WalkingPadConnection::Disconnected
                    };
                    publish_connection(&events, state, Some(error.message.clone()));
                    log_connect_error(&mut last_logged_error, &error.message);
                    let delay = Duration::from_millis(backoff.fail());
                    if wait_disconnected(&mut urgent, &mut normal, &events, delay).await {
                        return;
                    }
                    continue;
                }
                ConnectOutcome::Interrupted => continue,
                ConnectOutcome::Shutdown => return,
            };

        let (exit, was_healthy) = run_connected(&mut *pad, &mut urgent, &mut normal, &events).await;
        if let Err(error) = pad.disconnect().await {
            tracing::debug!(
                component = "walkingpad",
                error = %error,
                "WalkingPad disconnect failed"
            );
        }

        match exit {
            ConnectedExit::Shutdown => return,
            ConnectedExit::Reconnect(error) => {
                if was_healthy {
                    backoff.reset();
                    last_logged_error = None;
                }
                publish_connection(
                    &events,
                    WalkingPadConnection::Disconnected,
                    Some(error.clone()),
                );
                log_connect_error(&mut last_logged_error, &error);
                let delay = Duration::from_millis(backoff.fail());
                if wait_disconnected(&mut urgent, &mut normal, &events, delay).await {
                    return;
                }
            }
        }
    }
}

async fn connect_preemptible(
    backend: &mut dyn PadBackend,
    urgent: &mut mpsc::UnboundedReceiver<UrgentRequest>,
    normal: &mut mpsc::UnboundedReceiver<WalkingPadRequest>,
    events: &mpsc::UnboundedSender<RuntimeEvent>,
) -> ConnectOutcome {
    let connect = backend.connect();
    tokio::pin!(connect);
    tokio::select! {
        biased;
        request = urgent.recv() => match request {
            Some(UrgentRequest::Shutdown) | None => ConnectOutcome::Shutdown,
            Some(UrgentRequest::Stop) => {
                publish_command_failure(events, WalkingPadRequest::Stop, "DISCONNECTED");
                ConnectOutcome::Interrupted
            }
        },
        request = normal.recv() => match request {
            Some(request) => {
                publish_command_failure(events, request, "DISCONNECTED");
                ConnectOutcome::Interrupted
            }
            None => ConnectOutcome::Shutdown,
        },
        result = &mut connect => match result {
            Ok(pad) => ConnectOutcome::Connected(pad),
            Err(error) => ConnectOutcome::Failed(error),
        },
    }
}

async fn run_connected(
    pad: &mut dyn PadConnection,
    urgent: &mut mpsc::UnboundedReceiver<UrgentRequest>,
    normal: &mut mpsc::UnboundedReceiver<WalkingPadRequest>,
    events: &mpsc::UnboundedSender<RuntimeEvent>,
) -> (ConnectedExit, bool) {
    let mut last_status: Option<(WalkingPadTelemetry, Instant)> = None;
    let mut next_poll = Instant::now();
    let mut was_healthy = false;

    loop {
        tokio::select! {
            biased;
            request = urgent.recv() => match request {
                Some(UrgentRequest::Shutdown) | None => return (ConnectedExit::Shutdown, was_healthy),
                Some(UrgentRequest::Stop) => {
                    if let Err(error) = execute_stop(pad, events).await {
                        return (ConnectedExit::Reconnect(error), was_healthy);
                    }
                    next_poll = Instant::now();
                }
            },
            request = normal.recv() => match request {
                Some(request) => {
                    if let Err(error) = execute_normal_preemptible(
                        pad,
                        request,
                        last_status.as_ref(),
                        urgent,
                        events,
                    ).await {
                        return (error, was_healthy);
                    }
                    next_poll = Instant::now();
                }
                None => return (ConnectedExit::Shutdown, was_healthy),
            },
            _ = tokio::time::sleep_until(next_poll.into()) => {
                match poll_preemptible(pad, urgent, normal).await {
                    PollOutcome::Status(Ok(status)) => {
                        if !was_healthy {
                            tracing::info!(component = "walkingpad", "WalkingPad connected");
                            was_healthy = true;
                        }
                        last_status = Some((status.clone(), Instant::now()));
                        let _ = events.send(RuntimeEvent::WalkingPad(WalkingPadUpdate::Status {
                            telemetry: status,
                            received_at_ms: chrono::Utc::now().timestamp_millis(),
                        }));
                    }
                    PollOutcome::Status(Err(error)) => {
                        return (
                            ConnectedExit::Reconnect(format!("status failed: {error}")),
                            was_healthy,
                        );
                    }
                    PollOutcome::Urgent(UrgentRequest::Shutdown) => {
                        return (ConnectedExit::Shutdown, was_healthy);
                    }
                    PollOutcome::Urgent(UrgentRequest::Stop) => {
                        if let Err(error) = execute_stop(pad, events).await {
                            return (ConnectedExit::Reconnect(error), was_healthy);
                        }
                    }
                    PollOutcome::Normal(request) => {
                        if let Err(error) = execute_normal_preemptible(
                            pad,
                            request,
                            last_status.as_ref(),
                            urgent,
                            events,
                        ).await {
                            return (error, was_healthy);
                        }
                    }
                    PollOutcome::Closed => return (ConnectedExit::Shutdown, was_healthy),
                }
                next_poll = Instant::now() + POLL_INTERVAL;
            }
        }
    }
}

enum PollOutcome {
    Status(Result<WalkingPadTelemetry, String>),
    Urgent(UrgentRequest),
    Normal(WalkingPadRequest),
    Closed,
}

async fn poll_preemptible(
    pad: &mut dyn PadConnection,
    urgent: &mut mpsc::UnboundedReceiver<UrgentRequest>,
    normal: &mut mpsc::UnboundedReceiver<WalkingPadRequest>,
) -> PollOutcome {
    let outcome = {
        let status = pad.status(STATUS_TIMEOUT);
        tokio::pin!(status);
        tokio::select! {
            biased;
            request = urgent.recv() => request.map(PollOutcome::Urgent).unwrap_or(PollOutcome::Closed),
            request = normal.recv() => request.map(PollOutcome::Normal).unwrap_or(PollOutcome::Closed),
            result = &mut status => PollOutcome::Status(result),
        }
    };
    outcome
}

async fn execute_normal_preemptible(
    pad: &mut dyn PadConnection,
    request: WalkingPadRequest,
    last_status: Option<&(WalkingPadTelemetry, Instant)>,
    urgent: &mut mpsc::UnboundedReceiver<UrgentRequest>,
    events: &mpsc::UnboundedSender<RuntimeEvent>,
) -> Result<(), ConnectedExit> {
    if let Err(error) = validate_motion_request(request, last_status) {
        publish_command_failure(events, request, error);
        return Ok(());
    }

    enum Outcome {
        Command(Result<(), String>),
        Urgent(UrgentRequest),
        Closed,
    }

    let outcome = {
        let command = execute_normal(pad, request);
        tokio::pin!(command);
        tokio::select! {
            biased;
            urgent = urgent.recv() => urgent.map(Outcome::Urgent).unwrap_or(Outcome::Closed),
            result = &mut command => Outcome::Command(result),
        }
    };

    match outcome {
        Outcome::Command(Ok(())) => {
            let _ = events.send(RuntimeEvent::WalkingPad(
                WalkingPadUpdate::CommandSucceeded(request),
            ));
            Ok(())
        }
        Outcome::Command(Err(error)) => {
            publish_command_failure(events, request, error.clone());
            Err(ConnectedExit::Reconnect(error))
        }
        Outcome::Urgent(UrgentRequest::Stop) => execute_stop(pad, events)
            .await
            .map_err(ConnectedExit::Reconnect),
        Outcome::Urgent(UrgentRequest::Shutdown) | Outcome::Closed => Err(ConnectedExit::Shutdown),
    }
}

fn validate_motion_request(
    request: WalkingPadRequest,
    last_status: Option<&(WalkingPadTelemetry, Instant)>,
) -> Result<(), &'static str> {
    if request == WalkingPadRequest::Stop {
        return Ok(());
    }
    let Some((status, received)) = last_status else {
        return Err("NO FRESH STATUS");
    };
    if received.elapsed() > Duration::from_millis(STATUS_STALE_AFTER_MS as u64) {
        return Err("STATUS STALE");
    }
    match request {
        WalkingPadRequest::Start if status.speed_tenths == 0 => Ok(()),
        WalkingPadRequest::Start => Err("ALREADY MOVING"),
        WalkingPadRequest::SetSpeed(_) if status.speed_tenths == 0 => Err("START FIRST"),
        WalkingPadRequest::SetSpeed(_) => Ok(()),
        WalkingPadRequest::Stop => Ok(()),
    }
}

async fn execute_normal(
    pad: &mut dyn PadConnection,
    request: WalkingPadRequest,
) -> Result<(), String> {
    match request {
        WalkingPadRequest::Start => pad.start().await,
        WalkingPadRequest::SetSpeed(tenths) => pad.set_speed(tenths).await,
        WalkingPadRequest::Stop => unreachable!("stop uses the urgent channel"),
    }
}

async fn execute_stop(
    pad: &mut dyn PadConnection,
    events: &mpsc::UnboundedSender<RuntimeEvent>,
) -> Result<(), String> {
    let request = WalkingPadRequest::Stop;
    if let Err(error) = pad.halt().await {
        publish_command_failure(events, request, error.clone());
        return Err(error);
    }
    let _ = events.send(RuntimeEvent::WalkingPad(
        WalkingPadUpdate::CommandSucceeded(request),
    ));
    Ok(())
}

async fn wait_disconnected(
    urgent: &mut mpsc::UnboundedReceiver<UrgentRequest>,
    normal: &mut mpsc::UnboundedReceiver<WalkingPadRequest>,
    events: &mpsc::UnboundedSender<RuntimeEvent>,
    delay: Duration,
) -> bool {
    let retry = tokio::time::sleep(delay);
    tokio::pin!(retry);
    loop {
        tokio::select! {
            biased;
            request = urgent.recv() => match request {
                Some(UrgentRequest::Shutdown) | None => return true,
                Some(UrgentRequest::Stop) => publish_command_failure(
                    events,
                    WalkingPadRequest::Stop,
                    "DISCONNECTED",
                ),
            },
            request = normal.recv() => match request {
                Some(request) => publish_command_failure(events, request, "DISCONNECTED"),
                None => return true,
            },
            _ = &mut retry => return false,
        }
    }
}

fn publish_connection(
    events: &mpsc::UnboundedSender<RuntimeEvent>,
    state: WalkingPadConnection,
    error: Option<String>,
) {
    let _ = events.send(RuntimeEvent::WalkingPad(WalkingPadUpdate::Connection {
        state,
        error,
    }));
}

fn publish_command_failure(
    events: &mpsc::UnboundedSender<RuntimeEvent>,
    request: WalkingPadRequest,
    error: impl Into<String>,
) {
    let _ = events.send(RuntimeEvent::WalkingPad(WalkingPadUpdate::CommandFailed {
        request,
        error: error.into(),
    }));
}

fn log_connect_error(last: &mut Option<String>, error: &str) {
    if last.as_deref() == Some(error) {
        tracing::debug!(
            component = "walkingpad",
            error,
            "WalkingPad still unavailable"
        );
    } else {
        tracing::warn!(component = "walkingpad", error, "WalkingPad unavailable");
        *last = Some(error.to_string());
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use tokio::sync::Notify;

    use super::*;

    struct FakeBackend {
        connection: Option<FakeConnection>,
    }

    #[async_trait]
    impl PadBackend for FakeBackend {
        async fn connect(&mut self) -> Result<Box<dyn PadConnection>, ConnectFailure> {
            self.connection
                .take()
                .map(|connection| Box::new(connection) as Box<dyn PadConnection>)
                .ok_or_else(|| ConnectFailure::unavailable("no more fake connections"))
        }
    }

    struct BlockingBackend {
        entered: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[async_trait]
    impl PadBackend for BlockingBackend {
        async fn connect(&mut self) -> Result<Box<dyn PadConnection>, ConnectFailure> {
            self.entered.notify_one();
            self.release.notified().await;
            Err(ConnectFailure::unavailable("released"))
        }
    }

    struct FakeConnection {
        trace: Arc<Mutex<Vec<String>>>,
        statuses: Arc<Mutex<VecDeque<WalkingPadTelemetry>>>,
        block_status: Option<Arc<Notify>>,
    }

    #[async_trait]
    impl PadConnection for FakeConnection {
        async fn start(&mut self) -> Result<(), String> {
            self.trace.lock().unwrap().push("start".to_string());
            Ok(())
        }

        async fn halt(&mut self) -> Result<(), String> {
            self.trace.lock().unwrap().push("halt".to_string());
            Ok(())
        }

        async fn set_speed(&mut self, tenths: u8) -> Result<(), String> {
            self.trace.lock().unwrap().push(format!("speed:{tenths}"));
            Ok(())
        }

        async fn status(&mut self, _timeout: Duration) -> Result<WalkingPadTelemetry, String> {
            self.trace.lock().unwrap().push("poll".to_string());
            if let Some(notify) = &self.block_status {
                notify.notified().await;
            }
            self.statuses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| "no fake status".to_string())
        }

        async fn disconnect(self: Box<Self>) -> Result<(), String> {
            self.trace.lock().unwrap().push("disconnect".to_string());
            Ok(())
        }
    }

    fn status(speed_tenths: u8) -> WalkingPadTelemetry {
        WalkingPadTelemetry {
            counters: WalkingPadCounters::default(),
            speed_tenths,
            target_speed_tenths: speed_tenths,
            belt_state: u8::from(speed_tenths > 0),
            mode: WalkingPadMode::Manual,
        }
    }

    async fn wait_for(trace: &Arc<Mutex<Vec<String>>>, expected: &str) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if trace.lock().unwrap().iter().any(|entry| entry == expected) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("trace entry");
    }

    #[tokio::test]
    async fn stop_preempts_an_in_flight_status_poll() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let blocker = Arc::new(Notify::new());
        let connection = FakeConnection {
            trace: Arc::clone(&trace),
            statuses: Arc::new(Mutex::new(VecDeque::from([status(30)]))),
            block_status: Some(blocker),
        };
        let (events, _receiver) = mpsc::unbounded_channel();
        let service = WalkingPadService::spawn_with_backend(
            events,
            Box::new(FakeBackend {
                connection: Some(connection),
            }),
        );

        wait_for(&trace, "poll").await;
        service
            .controller
            .send(WalkingPadRequest::Stop)
            .expect("stop queued");
        wait_for(&trace, "halt").await;

        assert_eq!(trace.lock().unwrap()[..2], ["poll", "halt"]);
        service.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_cancels_an_in_flight_connection_attempt() {
        let entered = Arc::new(Notify::new());
        let (events, _receiver) = mpsc::unbounded_channel();
        let service = WalkingPadService::spawn_with_backend(
            events,
            Box::new(BlockingBackend {
                entered: Arc::clone(&entered),
                release: Arc::new(Notify::new()),
            }),
        );
        entered.notified().await;

        tokio::time::timeout(Duration::from_secs(1), service.shutdown())
            .await
            .expect("shutdown should cancel Bluetooth connection acquisition");
    }

    #[tokio::test]
    async fn stopped_belts_reject_speed_without_writing() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let connection = FakeConnection {
            trace: Arc::clone(&trace),
            statuses: Arc::new(Mutex::new(VecDeque::from([status(0), status(0)]))),
            block_status: None,
        };
        let (events, mut receiver) = mpsc::unbounded_channel();
        let service = WalkingPadService::spawn_with_backend(
            events,
            Box::new(FakeBackend {
                connection: Some(connection),
            }),
        );

        wait_for(&trace, "poll").await;
        service
            .controller
            .send(WalkingPadRequest::SetSpeed(34))
            .expect("speed queued");

        let failure = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(RuntimeEvent::WalkingPad(WalkingPadUpdate::CommandFailed {
                    request,
                    error,
                })) = receiver.recv().await
                {
                    return (request, error);
                }
            }
        })
        .await
        .expect("failure");
        assert_eq!(
            failure,
            (WalkingPadRequest::SetSpeed(34), "START FIRST".into())
        );
        assert!(!trace
            .lock()
            .unwrap()
            .iter()
            .any(|entry| entry == "speed:34"));
        service.shutdown().await;
    }
}
