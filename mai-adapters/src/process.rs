//! Adapter process lifecycle management.
//!
//! Each adapter runs as an isolated subprocess. `AdapterProcess` handles:
//! - Spawning the Python adapter runner with cgroups isolation
//! - JSON-RPC communication over stdin/stdout
//! - Crash detection and restart with exponential backoff
//! - Graceful shutdown with drain timeout

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::{debug, error, info, warn};

use crate::bridge::{RpcError, RpcRequest, RpcResponse};
use crate::config::{DiscoveredAdapter, FrameworkConfig};
use crate::errors::FrameworkError;

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
    /// Next RPC request ID.
    next_id: u64,
    /// Channel to send lines to the subprocess stdin.
    stdin_tx: Option<mpsc::Sender<String>>,
    /// Pending RPC requests awaiting responses.
    pending: Arc<Mutex<std::collections::HashMap<u64, PendingRequest>>>,
    /// Child process handle.
    child: Option<Child>,
    /// Channel for incoming events from the adapter.
    event_rx: Option<mpsc::Receiver<String>>,
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
            child: None,
            event_rx: None,
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
    pub async fn spawn(&mut self) -> Result<(), FrameworkError> {
        let name = &self.info.name;
        info!(adapter = %name, "Spawning adapter process");

        self.state = ProcessState::Starting;
        self.last_start = Some(Instant::now());

        let mut cmd = Command::new(self.config.python_path.as_os_str());
        cmd.arg(self.config.runner_script.as_os_str())
            .arg("--adapter-name")
            .arg(name)
            .arg("--module-path")
            .arg(self.info.module_path.as_os_str())
            .arg("--entry-module")
            .arg(&self.info.entry_module)
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

            // Replace cmd with cgroup-wrapped version
            cgroup_cmd
                .arg(self.config.python_path.as_os_str())
                .arg(self.config.runner_script.as_os_str())
                .arg("--adapter-name")
                .arg(name)
                .arg("--module-path")
                .arg(self.info.module_path.as_os_str())
                .arg("--entry-module")
                .arg(&self.info.entry_module)
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

        // Stdout reader task - reads lines and dispatches responses/events
        let pending_clone = Arc::clone(&self.pending);
        let adapter_name = name.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }

                // Try to parse as RpcResponse first
                if let Ok(response) = serde_json::from_str::<RpcResponse>(&line) {
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
                    // Treat as event (streaming tokens, heartbeats, etc.)
                    let _ = event_tx.send(line).await;
                }
            }
        });

        self.stdin_tx = Some(stdin_tx);
        self.child = Some(child);
        self.event_rx = Some(event_rx);

        info!(adapter = %name, "Adapter process spawned successfully");
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

    /// Take the event receiver (for the health monitor to consume).
    pub fn take_event_rx(&mut self) -> Option<mpsc::Receiver<String>> {
        self.event_rx.take()
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
    pub async fn check_alive(&mut self) -> bool {
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
