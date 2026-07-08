//! Adapter process lifecycle management.
//!
//! Each adapter runs as an isolated subprocess. `AdapterProcess` handles:
//! - Spawning the Python adapter runner with cgroups isolation
//! - JSON-RPC communication over stdin/stdout
//! - Crash detection and restart with exponential backoff
//! - Graceful shutdown with drain timeout

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::{debug, error, info, warn};

use crate::bridge::{
    HandshakeResponse, IpcEvent, IpcRequest, IpcStartupConfig, RpcError, RpcRequest, RpcResponse,
};
use crate::config::{DiscoveredAdapter, FrameworkConfig};
use crate::errors::FrameworkError;
use mai_hil::traits::AdapterCapabilities;

/// Maximum bytes in a single adapter stdout/stderr frame (finding F4). A hostile
/// or buggy adapter that never emits a newline must not grow the trusted
/// parent's memory without bound; the reader closes on an over-long frame.
const MAX_ADAPTER_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// Read one newline-delimited frame from `reader` into `buf`, bounded to
/// [`MAX_ADAPTER_FRAME_BYTES`]. Returns `Ok(None)` at EOF, `Ok(Some(len))` for a
/// complete frame (newline consumed, not included), or an `InvalidData` error
/// once the accumulated frame exceeds the cap (finding F4). Unlike
/// `AsyncBufReadExt::lines`, this never buffers an unbounded no-newline stream.
async fn read_bounded_frame<R>(reader: &mut R, buf: &mut Vec<u8>) -> std::io::Result<Option<usize>>
where
    R: AsyncBufReadExt + Unpin,
{
    buf.clear();
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            return Ok(if buf.is_empty() {
                None
            } else {
                Some(buf.len())
            });
        }
        if let Some(pos) = chunk.iter().position(|&b| b == b'\n') {
            buf.extend_from_slice(&chunk[..pos]);
            reader.consume(pos + 1);
            return Ok(Some(buf.len()));
        }
        buf.extend_from_slice(chunk);
        let consumed = chunk.len();
        reader.consume(consumed);
        if buf.len() > MAX_ADAPTER_FRAME_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "adapter frame exceeded maximum length",
            ));
        }
    }
}

/// State of an adapter process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProcessState {
    /// Not yet started.
    #[default]
    NotStarted,
    /// Process is starting up (initialize called, waiting for ready).
    Starting,
    /// Process is running and healthy.
    Running,
    /// Process has crashed, awaiting restart.
    Crashed,
    /// Process is being restarted (backoff period).
    Restarting,
    /// Process has been intentionally stopped.
    Stopped,
    /// Process exceeded max restarts and is permanently failed.
    Failed,
}

use serde::{Deserialize, Serialize};

impl std::fmt::Display for ProcessState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotStarted => write!(f, "not_started"),
            Self::Starting => write!(f, "starting"),
            Self::Running => write!(f, "running"),
            Self::Crashed => write!(f, "crashed"),
            Self::Restarting => write!(f, "restarting"),
            Self::Stopped => write!(f, "stopped"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

/// Pending request waiting for a response from the adapter process.
struct PendingRequest {
    response_tx: oneshot::Sender<Result<Value, RpcError>>,
}

type PendingIpcRequests = Arc<Mutex<HashMap<String, oneshot::Sender<Result<(), FrameworkError>>>>>;

/// Manages a single adapter subprocess.
pub struct AdapterProcess {
    /// Adapter discovery info.
    pub info: DiscoveredAdapter,
    /// Current process state.
    state: ProcessState,
    /// Framework configuration.
    config: Arc<FrameworkConfig>,
    /// Number of consecutive restarts.
    restart_count: u32,
    /// Last time the process was started.
    last_start: Option<Instant>,
    /// Last successful heartbeat timestamp.
    last_heartbeat: Option<Instant>,
    /// Next RPC request ID (legacy).
    next_id: u64,
    /// Channel to send lines to the subprocess stdin.
    stdin_tx: Option<mpsc::Sender<String>>,
    /// Pending RPC requests awaiting responses (legacy JSON-RPC).
    pending: Arc<Mutex<std::collections::HashMap<u64, PendingRequest>>>,
    /// Pending IPC requests awaiting done/error events (NDJSON protocol).
    pending_ipc: PendingIpcRequests,
    /// Child process handle.
    child: Option<Child>,
    /// Channel for incoming events from the adapter (raw lines).
    event_rx: Option<mpsc::Receiver<String>>,
    /// Channel for typed IPC events (token stream, usage, etc.).
    ipc_event_rx: Option<mpsc::Receiver<IpcEvent>>,
    /// Adapter handle string from handshake.
    pub handle: Option<String>,
    /// Capabilities reported by the adapter during handshake.
    pub capabilities: Option<AdapterCapabilities>,
}

impl AdapterProcess {
    /// Create a new adapter process manager (not yet started).
    pub fn new(info: DiscoveredAdapter, config: Arc<FrameworkConfig>) -> Self {
        Self {
            info,
            state: ProcessState::NotStarted,
            config,
            restart_count: 0,
            last_start: None,
            last_heartbeat: None,
            next_id: 1,
            stdin_tx: None,
            pending: Arc::new(Mutex::new(std::collections::HashMap::new())),
            pending_ipc: Arc::new(Mutex::new(std::collections::HashMap::new())),
            child: None,
            event_rx: None,
            ipc_event_rx: None,
            handle: None,
            capabilities: None,
        }
    }

    /// Get current process state.
    pub fn state(&self) -> ProcessState {
        self.state
    }

    /// Get restart count.
    pub fn restart_count(&self) -> u32 {
        self.restart_count
    }

    /// Get time since last heartbeat (None if never received).
    pub fn time_since_heartbeat(&self) -> Option<Duration> {
        self.last_heartbeat.map(|t| t.elapsed())
    }

    /// Record that a heartbeat was received.
    pub fn record_heartbeat(&mut self) {
        self.last_heartbeat = Some(Instant::now());
    }

    /// Spawn the adapter subprocess.
    #[allow(clippy::too_many_lines)]
    pub async fn spawn(&mut self) -> Result<(), FrameworkError> {
        let name = &self.info.name;
        info!(adapter = %name, "Spawning adapter process");

        self.state = ProcessState::Starting;
        self.last_start = Some(Instant::now());

        // IPC Protocol v1.0: single positional arg (adapter name only).
        // Module path and class are sent via startup config on stdin.
        let mut cmd = Command::new(self.config.python_path.as_os_str());
        cmd.arg(self.config.runner_script.as_os_str())
            .arg(name) // Single positional arg per IPC-PROTOCOL.md
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // Don't inherit parent's env for isolation
            .env_clear()
            .env("PYTHONPATH", self.config.adapters_dir.as_os_str())
            .env("MAI_ADAPTER_NAME", name)
            .env("MAI_LOG_LEVEL", "INFO");

        // Apply cgroups limits if configured (Linux only)
        #[cfg(target_os = "linux")]
        if self.config.cgroup_memory_limit > 0 || self.config.cgroup_cpu_quota > 0 {
            // cgroups v2 systemd-run wrapper for process isolation
            let mut cgroup_cmd = Command::new("systemd-run");
            cgroup_cmd
                .arg("--user")
                .arg("--scope")
                .arg(format!("--unit=mai-adapter-{name}"));

            if self.config.cgroup_memory_limit > 0 {
                cgroup_cmd.arg(format!(
                    "--property=MemoryMax={}",
                    self.config.cgroup_memory_limit
                ));
            }
            if self.config.cgroup_cpu_quota > 0 {
                cgroup_cmd.arg(format!(
                    "--property=CPUQuota={}%",
                    self.config.cgroup_cpu_quota / 10_000 // Convert microseconds to percent
                ));
            }

            // Replace cmd with cgroup-wrapped version (single positional arg)
            cgroup_cmd
                .arg(self.config.python_path.as_os_str())
                .arg(self.config.runner_script.as_os_str())
                .arg(name) // Single positional arg per IPC-PROTOCOL.md
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());

            cmd = cgroup_cmd;
        }

        let mut child = cmd.spawn().map_err(|e| FrameworkError::SpawnFailed {
            name: name.clone(),
            reason: e.to_string(),
        })?;

        // Set up stdin writer channel
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| FrameworkError::SpawnFailed {
                name: name.clone(),
                reason: "Failed to capture stdin".to_string(),
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| FrameworkError::SpawnFailed {
                name: name.clone(),
                reason: "Failed to capture stdout".to_string(),
            })?;

        // Drain stderr so a chatty adapter cannot fill the OS pipe buffer and
        // deadlock on write (finding F4). Frames are bounded like stdout.
        if let Some(stderr) = child.stderr.take() {
            let adapter = name.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut buf: Vec<u8> = Vec::with_capacity(1024);
                loop {
                    match read_bounded_frame(&mut reader, &mut buf).await {
                        Ok(None) => break,
                        Ok(Some(_)) => {
                            let text = String::from_utf8_lossy(&buf);
                            let text = text.trim();
                            if !text.is_empty() {
                                debug!(adapter = %adapter, "adapter stderr: {text}");
                            }
                        }
                        Err(e) => {
                            warn!(adapter = %adapter, error = %e, "adapter stderr frame error");
                            break;
                        }
                    }
                }
            });
        }

        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(64);
        let (event_tx, event_rx) = mpsc::channel::<String>(256);

        // Stdin writer task
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(line) = stdin_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                if stdin.write_all(b"\n").await.is_err() {
                    break;
                }
                let _ = stdin.flush().await;
            }
        });

        // IPC event channel for typed NDJSON events (tokens, usage, etc.)
        let (ipc_event_tx, ipc_event_rx) = mpsc::channel::<IpcEvent>(256);

        // Stdout reader task - reads NDJSON lines and dispatches:
        //   - IpcEvent lines to the ipc_event channel (for streaming consumers)
        //   - Legacy RpcResponse lines to the pending map (backward compat)
        //   - Unknown lines to the raw event channel
        let pending_clone = Arc::clone(&self.pending);
        let adapter_name = name.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut buf: Vec<u8> = Vec::with_capacity(4096);

            loop {
                match read_bounded_frame(&mut reader, &mut buf).await {
                    Ok(None) => break, // EOF
                    Ok(Some(_)) => {}
                    Err(e) => {
                        warn!(adapter = %adapter_name, error = %e, "adapter stdout frame error; closing reader");
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&buf);
                let line = text.trim();
                if line.is_empty() {
                    continue;
                }

                // Try IPC NDJSON event first (has "type" and "request_id" fields)
                if let Ok(ipc_event) = serde_json::from_str::<IpcEvent>(line) {
                    let _ = ipc_event_tx.send(ipc_event).await;
                }
                // Fallback: legacy RpcResponse (has "id" field)
                else if let Ok(response) = serde_json::from_str::<RpcResponse>(line) {
                    let mut pending = pending_clone.lock().await;
                    if let Some(req) = pending.remove(&response.id) {
                        let result = if let Some(err) = response.error {
                            Err(err)
                        } else {
                            Ok(response.result.unwrap_or(Value::Null))
                        };
                        let _ = req.response_tx.send(result);
                    } else {
                        debug!(
                            adapter = %adapter_name,
                            id = response.id,
                            "Response for unknown request ID"
                        );
                    }
                } else {
                    // Unknown format - send as raw event
                    let _ = event_tx.send(line.to_string()).await;
                }
            }
        });

        self.stdin_tx = Some(stdin_tx.clone());
        self.child = Some(child);
        self.event_rx = Some(event_rx);
        self.ipc_event_rx = Some(ipc_event_rx);

        // ── IPC Startup Handshake (Protocol v1.0) ────────────────────────
        // Step 1: Send startup config as first stdin line
        let startup_config = IpcStartupConfig {
            adapter_name: name.clone(),
            module_path: self.info.module_path.to_string_lossy().to_string(),
            entry_class: self.info.entry_module.clone(),
            config: None, // Adapter-specific config added by manager
        };
        let config_json =
            serde_json::to_string(&startup_config).map_err(|e| FrameworkError::ProtocolError {
                name: name.clone(),
                detail: format!("Failed to serialize startup config: {e}"),
            })?;
        stdin_tx
            .send(config_json)
            .await
            .map_err(|_| FrameworkError::SpawnFailed {
                name: name.clone(),
                reason: "stdin channel closed before startup config sent".to_string(),
            })?;

        info!(adapter = %name, "Adapter process spawned, startup config sent");
        Ok(())
    }

    /// Send an RPC request and await the response.
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, FrameworkError> {
        let name = &self.info.name;

        if self.state != ProcessState::Running && self.state != ProcessState::Starting {
            return Err(FrameworkError::NotReady {
                name: name.clone(),
                state: self.state.to_string(),
            });
        }

        let id = self.next_id;
        self.next_id += 1;

        let request = RpcRequest {
            id,
            method: method.to_string(),
            params,
        };

        let request_json = serde_json::to_string(&request)?;

        // Register pending request
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, PendingRequest { response_tx: tx });
        }

        // Send request
        let stdin_tx = self
            .stdin_tx
            .as_ref()
            .ok_or_else(|| FrameworkError::NotReady {
                name: name.clone(),
                state: "no stdin channel".to_string(),
            })?;

        stdin_tx
            .send(request_json)
            .await
            .map_err(|_| FrameworkError::Io {
                name: name.clone(),
                source: std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stdin channel closed"),
            })?;

        // Await response with timeout
        let timeout = Duration::from_millis(self.config.request_timeout_ms);
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(rpc_err))) => Err(FrameworkError::ProtocolError {
                name: name.clone(),
                detail: format!("[{}] {}", rpc_err.code, rpc_err.detail),
            }),
            Ok(Err(_)) => Err(FrameworkError::ProcessCrashed {
                name: name.clone(),
                exit_code: None,
            }),
            Err(_) => {
                // Remove the pending request on timeout
                let mut pending = self.pending.lock().await;
                pending.remove(&id);
                Err(FrameworkError::ResponseTimeout {
                    name: name.clone(),
                    timeout_ms: self.config.request_timeout_ms,
                })
            }
        }
    }

    /// Take the raw event receiver (for the health monitor to consume).
    pub fn take_event_rx(&mut self) -> Option<mpsc::Receiver<String>> {
        self.event_rx.take()
    }

    /// Take the typed IPC event receiver (for streaming token consumers).
    pub fn take_ipc_event_rx(&mut self) -> Option<mpsc::Receiver<IpcEvent>> {
        self.ipc_event_rx.take()
    }

    /// Wait for the handshake response from the subprocess.
    /// Must be called after spawn(). Blocks up to 30 seconds (per IPC-PROTOCOL.md).
    /// On success, caches capabilities and handle, then marks process Running.
    pub async fn await_handshake(&mut self) -> Result<HandshakeResponse, FrameworkError> {
        let name = &self.info.name;
        let timeout = Duration::from_secs(30);

        // Read from the ipc_event channel; the first IPC line should be the handshake
        let ipc_rx = self
            .ipc_event_rx
            .as_mut()
            .ok_or_else(|| FrameworkError::NotReady {
                name: name.clone(),
                state: "no ipc event channel".to_string(),
            })?;

        let handshake_event = tokio::time::timeout(timeout, ipc_rx.recv())
            .await
            .map_err(|_| FrameworkError::InitFailed {
                name: name.clone(),
                reason: "Handshake timeout (30s) - no response from adapter".to_string(),
            })?
            .ok_or_else(|| FrameworkError::ProcessCrashed {
                name: name.clone(),
                exit_code: None,
            })?;

        // The handshake event has type "handshake"
        if handshake_event.event_type != "handshake" {
            return Err(FrameworkError::ProtocolError {
                name: name.clone(),
                detail: format!(
                    "Expected handshake event, got type '{}'",
                    handshake_event.event_type
                ),
            });
        }

        // Reconstruct full JSON from the IpcEvent to deserialize as HandshakeResponse
        let mut full_obj = serde_json::Map::new();
        full_obj.insert(
            "type".to_string(),
            Value::String(handshake_event.event_type.clone()),
        );
        full_obj.insert(
            "request_id".to_string(),
            Value::String(handshake_event.request_id.clone()),
        );
        for (k, v) in &handshake_event.data {
            full_obj.insert(k.clone(), v.clone());
        }

        let handshake: HandshakeResponse = serde_json::from_value(Value::Object(full_obj))
            .map_err(|e| FrameworkError::ProtocolError {
                name: name.clone(),
                detail: format!("Invalid handshake response: {e}"),
            })?;

        info!(
            adapter = %name,
            version = %handshake.version,
            handle = %handshake.handle,
            "Handshake complete"
        );

        self.handle = Some(handshake.handle.clone());
        self.capabilities = Some(handshake.capabilities.clone());
        self.mark_running();

        Ok(handshake)
    }

    /// Send an IPC request (NDJSON protocol v1.0).
    /// Returns the request_id for callers that want to correlate events.
    pub async fn send_ipc(
        &self,
        request_type: &str,
        payload: Value,
    ) -> Result<String, FrameworkError> {
        let name = &self.info.name;

        if self.state != ProcessState::Running {
            return Err(FrameworkError::NotReady {
                name: name.clone(),
                state: self.state.to_string(),
            });
        }

        let request_id = uuid::Uuid::new_v4().to_string();
        let request = IpcRequest {
            request_id: request_id.clone(),
            request_type: request_type.to_string(),
            payload,
        };

        let request_json =
            serde_json::to_string(&request).map_err(|e| FrameworkError::ProtocolError {
                name: name.clone(),
                detail: format!("Failed to serialize IPC request: {e}"),
            })?;

        let stdin_tx = self
            .stdin_tx
            .as_ref()
            .ok_or_else(|| FrameworkError::NotReady {
                name: name.clone(),
                state: "no stdin channel".to_string(),
            })?;

        stdin_tx
            .send(request_json)
            .await
            .map_err(|_| FrameworkError::Io {
                name: name.clone(),
                source: std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stdin channel closed"),
            })?;

        debug!(adapter = %name, request_id = %request_id, method = %request_type, "IPC request sent");
        Ok(request_id)
    }

    /// Mark the process as running (after successful initialization).
    pub fn mark_running(&mut self) {
        self.state = ProcessState::Running;
        self.restart_count = 0;
        self.last_heartbeat = Some(Instant::now());
    }

    /// Mark the process as crashed and compute backoff duration.
    pub fn mark_crashed(&mut self) -> Option<Duration> {
        self.state = ProcessState::Crashed;
        self.restart_count += 1;

        if self.restart_count > self.config.max_restart_attempts {
            self.state = ProcessState::Failed;
            error!(
                adapter = %self.info.name,
                attempts = self.restart_count,
                "Max restart attempts exceeded, adapter permanently failed"
            );
            return None;
        }

        // Exponential backoff: base * 2^(restart_count - 1), capped at max
        let backoff_ms = self
            .config
            .base_backoff_ms
            .saturating_mul(1u64 << (self.restart_count - 1).min(6));
        let backoff_ms = backoff_ms.min(self.config.max_backoff_ms);

        warn!(
            adapter = %self.info.name,
            restart_count = self.restart_count,
            backoff_ms = backoff_ms,
            "Adapter crashed, will restart after backoff"
        );

        self.state = ProcessState::Restarting;
        Some(Duration::from_millis(backoff_ms))
    }

    /// Kill the subprocess if still running.
    pub async fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        self.stdin_tx = None;
        self.state = ProcessState::Stopped;
    }

    /// Gracefully shutdown: send shutdown RPC, wait up to 5s, then kill.
    pub async fn shutdown(&mut self) -> Result<(), FrameworkError> {
        if self.state != ProcessState::Running {
            self.kill().await;
            return Ok(());
        }

        // Send shutdown command
        let shutdown_result =
            tokio::time::timeout(Duration::from_secs(5), self.call("shutdown", Value::Null)).await;

        match shutdown_result {
            Ok(Ok(_)) => {
                info!(adapter = %self.info.name, "Adapter shut down gracefully");
            }
            _ => {
                warn!(adapter = %self.info.name, "Graceful shutdown timed out, killing process");
            }
        }

        self.kill().await;
        Ok(())
    }

    /// Check if the child process has exited.
    pub fn check_alive(&mut self) -> bool {
        if let Some(ref mut child) = self.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let code = status.code();
                    warn!(adapter = %self.info.name, exit_code = ?code, "Process exited");
                    false
                }
                Ok(None) => true, // Still running
                Err(e) => {
                    error!(adapter = %self.info.name, error = %e, "Error checking process status");
                    false
                }
            }
        } else {
            false
        }
    }
}

#[cfg(test)]
mod frame_tests {
    use super::read_bounded_frame;
    use tokio::io::BufReader;

    #[tokio::test]
    async fn bounded_frame_reads_lines_then_eof() {
        let data = b"one\ntwo\nthree";
        let mut r = BufReader::new(&data[..]);
        let mut buf = Vec::new();
        assert_eq!(read_bounded_frame(&mut r, &mut buf).await.unwrap(), Some(3));
        assert_eq!(&buf, b"one");
        assert_eq!(read_bounded_frame(&mut r, &mut buf).await.unwrap(), Some(3));
        assert_eq!(&buf, b"two");
        // Final frame has no trailing newline.
        assert_eq!(read_bounded_frame(&mut r, &mut buf).await.unwrap(), Some(5));
        assert_eq!(&buf, b"three");
        assert_eq!(read_bounded_frame(&mut r, &mut buf).await.unwrap(), None);
    }

    #[tokio::test]
    async fn bounded_frame_rejects_oversized() {
        // A no-newline stream larger than the cap is refused, not buffered.
        let big = vec![b'x'; super::MAX_ADAPTER_FRAME_BYTES + 16];
        let mut r = BufReader::new(&big[..]);
        let mut buf = Vec::new();
        let err = read_bounded_frame(&mut r, &mut buf).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }
}
