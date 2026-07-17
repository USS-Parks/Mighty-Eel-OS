//! Production composition for governed AOG tool execution.
//!
//! This crate closes the boundary left deliberately abstract by
//! `aog-toolproxy`: a caller must present a server-established
//! [`VerifiedRequestContext`], mutations block on the real approval inbox, each
//! call receives a tenant/tool-specific OpenBao token, and execution can reach
//! only an operator-configured HTTP endpoint. No caller-provided URL, token
//! role, tenant, or approval identity reaches an authority-bearing sink.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use aog_approvals::{ApprovalInbox, InboxGate};
use aog_toolproxy::{
    CredentialMinter, InvokeContext, MintedCredential, ProxyError, ToolExecutor, ToolProxy,
};
use async_trait::async_trait;
use fabric_contracts::{RequestOperation, VerifiedRequestContext};
use futures_util::StreamExt;
use mai_agent::types::{ToolAccessRole, ToolCall, ToolDefinition, ToolResult};
use reqwest::{Client, Url, redirect::Policy};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use wsf_bridge::{OpenBaoAuth, OpenBaoError};

/// Default response ceiling for an external tool (one MiB).
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("runtime configuration: {0}")]
    Config(String),
    #[error("request context is not authorized for this tool invocation: {0}")]
    Authority(String),
    #[error("tool proxy: {0}")]
    Proxy(#[from] ProxyError),
    #[error("OpenBao: {0}")]
    OpenBao(#[from] OpenBaoError),
    #[error("revocation spool: {0}")]
    Spool(#[from] std::io::Error),
}

/// Operator-owned production configuration. Roles are keyed first by verified
/// tenant and then by exact registered tool id; routes are keyed only by exact
/// tool id. Missing mappings fail closed before any request leaves AOG.
pub struct RuntimeConfig {
    pub openbao: OpenBaoAuth,
    pub token_roles: BTreeMap<String, BTreeMap<String, TokenRoleBinding>>,
    pub tool_routes: BTreeMap<String, String>,
    pub revocation_spool: PathBuf,
    pub max_response_bytes: usize,
}

/// Exact OpenBao authority attached to one verified tenant/tool pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRoleBinding {
    pub role: String,
    pub policies: Vec<String>,
}

impl RuntimeConfig {
    #[must_use]
    pub fn new(openbao: OpenBaoAuth, revocation_spool: impl Into<PathBuf>) -> Self {
        Self {
            openbao,
            token_roles: BTreeMap::new(),
            tool_routes: BTreeMap::new(),
            revocation_spool: revocation_spool.into(),
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
        }
    }
}

/// The production caller/executor seam used by AOG API or controller code.
pub struct ProductionToolRuntime {
    proxy: ToolProxy,
    approvals: Arc<ApprovalInbox>,
    executor: HttpToolExecutor,
    revocations: Arc<RevocationQueue>,
}

impl ProductionToolRuntime {
    /// Assemble the complete fail-closed path and start the durable revocation
    /// worker. Construction requires an active Tokio runtime.
    pub fn new(config: RuntimeConfig) -> Result<Self, RuntimeError> {
        if config.max_response_bytes == 0 {
            return Err(RuntimeError::Config(
                "max_response_bytes must be non-zero".to_string(),
            ));
        }
        validate_role_map(&config.token_roles)?;
        let executor = HttpToolExecutor::new(&config.tool_routes, config.max_response_bytes)?;
        let revocations = Arc::new(RevocationQueue::new(
            config.openbao.clone(),
            config.revocation_spool,
        )?);
        revocations.start()?;
        let minter = OpenBaoCredentialMinter {
            openbao: config.openbao,
            roles: config.token_roles,
            revocations: Arc::clone(&revocations),
        };
        let approvals = Arc::new(ApprovalInbox::new());
        let proxy = ToolProxy::new()
            .with_minter(Box::new(minter))
            .with_approvals(Box::new(InboxGate(Arc::clone(&approvals))));
        Ok(Self {
            proxy,
            approvals,
            executor,
            revocations,
        })
    }

    pub fn register(&self, tool: ToolDefinition) -> Result<(), RuntimeError> {
        self.proxy.register(tool).map_err(RuntimeError::from)
    }

    /// Invoke an exact registered tool using only server-established identity,
    /// operation, tenant, and canonical resource bindings.
    pub async fn invoke_verified(
        &self,
        request: &VerifiedRequestContext,
        session_id: &str,
        call: &ToolCall,
    ) -> Result<ToolResult, RuntimeError> {
        validate_session_id(session_id)?;
        request
            .require_operation(RequestOperation::AogToolInvoke)
            .map_err(|error| RuntimeError::Authority(error.to_string()))?;
        if request.resource().kind() != "tool" || request.resource().name() != call.tool_id {
            return Err(RuntimeError::Authority(
                "canonical resource is not the requested tool".to_string(),
            ));
        }
        let principal = request.principal();
        if principal.tenant_id.trim().is_empty() || principal.token_lineage.is_none() {
            return Err(RuntimeError::Authority(
                "tool invocation requires tenant and token lineage".to_string(),
            ));
        }
        let role = role_from_authenticated_claims(&principal.roles);
        let context = InvokeContext::from_verified_request(session_id, role, request);
        self.proxy
            .invoke(call, &context, &self.executor)
            .await
            .map_err(RuntimeError::from)
    }

    #[must_use]
    pub fn approvals(&self) -> &Arc<ApprovalInbox> {
        &self.approvals
    }

    #[must_use]
    pub fn receipts(&self) -> Vec<aog_toolproxy::receipt::ToolReceipt> {
        self.proxy.receipts()
    }

    #[must_use]
    pub fn verify_receipts(&self) -> bool {
        self.proxy.verify_receipts()
    }

    /// Number of accessor-only revocation records awaiting remote completion.
    pub fn pending_revocations(&self) -> Result<usize, RuntimeError> {
        self.revocations.pending().map_err(RuntimeError::from)
    }

    /// Drain queued revocations immediately. Primarily an operator/readiness
    /// gate; the background worker also drains on construction and notification.
    pub async fn drain_revocations(&self) -> Result<(), RuntimeError> {
        self.revocations.drain().await
    }
}

fn validate_session_id(session_id: &str) -> Result<(), RuntimeError> {
    if session_id.is_empty()
        || session_id.len() > 128
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(RuntimeError::Authority(
            "session id must match [A-Za-z0-9._:-]{1,128}".to_string(),
        ));
    }
    Ok(())
}

fn role_from_authenticated_claims(roles: &[String]) -> ToolAccessRole {
    if roles.iter().any(|role| role == "aog:tool:admin") {
        ToolAccessRole::Admin
    } else if roles.iter().any(|role| role == "aog:tool:parent") {
        ToolAccessRole::Parent
    } else if roles.iter().any(|role| role == "aog:tool:teen") {
        ToolAccessRole::Teen
    } else if roles.iter().any(|role| role == "aog:tool:child") {
        ToolAccessRole::Child
    } else {
        ToolAccessRole::Guest
    }
}

fn validate_role_map(
    roles: &BTreeMap<String, BTreeMap<String, TokenRoleBinding>>,
) -> Result<(), RuntimeError> {
    if roles.is_empty() {
        return Err(RuntimeError::Config(
            "at least one tenant/tool token-role mapping is required".to_string(),
        ));
    }
    for (tenant, tools) in roles {
        if tenant.trim().is_empty() || tools.is_empty() {
            return Err(RuntimeError::Config(
                "token-role tenants and tool maps must be non-empty".to_string(),
            ));
        }
        for (tool, binding) in tools {
            if tool.trim().is_empty()
                || binding.role.is_empty()
                || binding.role.len() > 128
                || !binding
                    .role
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                || binding.policies.is_empty()
                || binding.policies.iter().any(|policy| {
                    policy.is_empty()
                        || policy.len() > 128
                        || !policy
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                })
            {
                return Err(RuntimeError::Config(format!(
                    "invalid token role mapping for tenant {tenant} tool {tool}"
                )));
            }
        }
    }
    Ok(())
}

struct OpenBaoCredentialMinter {
    openbao: OpenBaoAuth,
    roles: BTreeMap<String, BTreeMap<String, TokenRoleBinding>>,
    revocations: Arc<RevocationQueue>,
}

#[async_trait]
impl CredentialMinter for OpenBaoCredentialMinter {
    async fn mint(
        &self,
        tool: &ToolDefinition,
        context: &InvokeContext,
        authority_ttl: Duration,
    ) -> Result<MintedCredential, String> {
        let tenant = context
            .tenant_id()
            .ok_or_else(|| "verified tenant is required".to_string())?;
        let binding = self
            .roles
            .get(tenant)
            .and_then(|tools| tools.get(&tool.id))
            .ok_or_else(|| {
                format!(
                    "no token role configured for tenant {tenant} tool {}",
                    tool.id
                )
            })?;
        let parent = self
            .openbao
            .login()
            .await
            .map_err(|error| error.to_string())?;
        let mut metadata = BTreeMap::new();
        metadata.insert("tenant".to_string(), tenant.to_string());
        metadata.insert("tool".to_string(), tool.id.clone());
        metadata.insert("session".to_string(), context.session_id.clone());
        let display_name = format!(
            "aog-{}",
            blake3::hash(context.session_id.as_bytes()).to_hex()
        );
        let lease = self
            .openbao
            .create_token_for_role(
                &parent,
                &binding.role,
                &display_name,
                authority_ttl,
                &binding.policies,
                &metadata,
            )
            .await
            .map_err(|error| error.to_string())?;
        let (secret, accessor, lease_duration) = lease.into_parts();
        let expires_at = SystemTime::now()
            .checked_add(lease_duration)
            .ok_or_else(|| "credential expiry overflow".to_string())?;
        Ok(MintedCredential {
            lease_id: accessor,
            secret,
            expires_at,
        })
    }

    fn revoke(&self, lease_id: &str) {
        if let Err(error) = self.revocations.enqueue(lease_id) {
            tracing::error!(error = %error, "failed to durably enqueue OpenBao token revocation");
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct RevocationRecord {
    accessor: String,
    enqueued_at: String,
}

struct RevocationQueue {
    openbao: OpenBaoAuth,
    dir: PathBuf,
    notify: Notify,
    drain_lock: tokio::sync::Mutex<()>,
}

impl RevocationQueue {
    fn new(openbao: OpenBaoAuth, dir: PathBuf) -> Result<Self, std::io::Error> {
        fs::create_dir_all(&dir)?;
        let probe = dir.join(".write-probe");
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&probe)?;
        fs::remove_file(probe)?;
        Ok(Self {
            openbao,
            dir,
            notify: Notify::new(),
            drain_lock: tokio::sync::Mutex::new(()),
        })
    }

    fn start(self: &Arc<Self>) -> Result<(), RuntimeError> {
        let handle = tokio::runtime::Handle::try_current().map_err(|_| {
            RuntimeError::Config("construction requires an active Tokio runtime".to_string())
        })?;
        let queue = Arc::clone(self);
        handle.spawn(async move {
            loop {
                if let Err(error) = queue.drain().await {
                    tracing::warn!(error = %error, "OpenBao revocation drain deferred");
                }
                queue.notify.notified().await;
            }
        });
        Ok(())
    }

    fn enqueue(&self, accessor: &str) -> Result<(), std::io::Error> {
        let name = format!("{}.json", blake3::hash(accessor.as_bytes()).to_hex());
        let final_path = self.dir.join(name);
        if final_path.exists() {
            self.notify.notify_one();
            return Ok(());
        }
        let mut temp_name = String::from(".pending-");
        write!(
            &mut temp_name,
            "{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        )
        .expect("write to string");
        let temp_path = self.dir.join(temp_name);
        let record = serde_json::to_vec(&RevocationRecord {
            accessor: accessor.to_string(),
            enqueued_at: chrono::Utc::now().to_rfc3339(),
        })
        .map_err(std::io::Error::other)?;
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)?;
        file.write_all(&record)?;
        file.sync_all()?;
        fs::rename(&temp_path, &final_path)?;
        self.notify.notify_one();
        Ok(())
    }

    fn records(&self) -> Result<Vec<(PathBuf, RevocationRecord)>, std::io::Error> {
        let mut records = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let raw = fs::read(&path)?;
            let record = serde_json::from_slice(&raw).map_err(std::io::Error::other)?;
            records.push((path, record));
        }
        records.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(records)
    }

    fn pending(&self) -> Result<usize, std::io::Error> {
        Ok(self.records()?.len())
    }

    async fn drain(&self) -> Result<(), RuntimeError> {
        let _guard = self.drain_lock.lock().await;
        let records = self.records()?;
        if records.is_empty() {
            return Ok(());
        }
        let parent = self.openbao.login().await?;
        for (path, record) in records {
            self.openbao
                .revoke_token_accessor(&parent, &record.accessor)
                .await?;
            fs::remove_file(path)?;
        }
        Ok(())
    }
}

struct EndpointRoute {
    url: Url,
}

struct HttpToolExecutor {
    client: Client,
    routes: BTreeMap<String, EndpointRoute>,
    max_response_bytes: usize,
}

impl HttpToolExecutor {
    fn new(
        routes: &BTreeMap<String, String>,
        max_response_bytes: usize,
    ) -> Result<Self, RuntimeError> {
        if routes.is_empty() {
            return Err(RuntimeError::Config(
                "at least one exact tool route is required".to_string(),
            ));
        }
        let routes = routes
            .iter()
            .map(|(tool, raw)| {
                if tool.trim().is_empty() {
                    return Err(RuntimeError::Config("tool route id is empty".to_string()));
                }
                let url = Url::parse(raw).map_err(|error| {
                    RuntimeError::Config(format!("invalid route for {tool}: {error}"))
                })?;
                validate_endpoint(tool, &url)?;
                Ok((tool.clone(), EndpointRoute { url }))
            })
            .collect::<Result<_, _>>()?;
        let client = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|error| RuntimeError::Config(format!("HTTP client: {error}")))?;
        Ok(Self {
            client,
            routes,
            max_response_bytes,
        })
    }
}

fn validate_endpoint(tool: &str, url: &Url) -> Result<(), RuntimeError> {
    if url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(RuntimeError::Config(format!(
            "route for {tool} may not contain userinfo, query, or fragment"
        )));
    }
    let loopback = url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(RuntimeError::Config(format!(
            "route for {tool} must use HTTPS or loopback HTTP"
        )));
    }
    Ok(())
}

#[async_trait]
impl ToolExecutor for HttpToolExecutor {
    async fn execute(
        &self,
        tool: &ToolDefinition,
        call: &ToolCall,
        credential: Option<&MintedCredential>,
    ) -> ToolResult {
        let started = Instant::now();
        let result = self.execute_inner(tool, call, credential).await;
        match result {
            Ok(output) => ToolResult {
                call_id: call.call_id.clone(),
                tool_id: call.tool_id.clone(),
                success: true,
                output,
                error: None,
                duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            },
            Err(error) => ToolResult {
                call_id: call.call_id.clone(),
                tool_id: call.tool_id.clone(),
                success: false,
                output: serde_json::Value::Null,
                error: Some(error),
                duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            },
        }
    }
}

impl HttpToolExecutor {
    async fn execute_inner(
        &self,
        tool: &ToolDefinition,
        call: &ToolCall,
        credential: Option<&MintedCredential>,
    ) -> Result<serde_json::Value, String> {
        let route = self
            .routes
            .get(&tool.id)
            .ok_or_else(|| "tool has no operator-configured executor route".to_string())?;
        let credential =
            credential.ok_or_else(|| "call-scoped credential is required".to_string())?;
        let response = self
            .client
            .post(route.url.clone())
            .bearer_auth(&credential.secret)
            .json(&call.arguments)
            .send()
            .await
            .map_err(|error| format!("tool transport: {error}"))?;
        let status = response.status();
        if response.content_length().is_some_and(|length| {
            length > u64::try_from(self.max_response_bytes).unwrap_or(u64::MAX)
        }) {
            return Err("tool response exceeds configured byte ceiling".to_string());
        }
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| format!("tool response: {error}"))?;
            if body.len().saturating_add(chunk.len()) > self.max_response_bytes {
                return Err("tool response exceeds configured byte ceiling".to_string());
            }
            body.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            return Err(format!("tool returned HTTP {status}"));
        }
        if body.is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_slice(&body).map_err(|_| "tool returned invalid JSON".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_policy_is_fail_closed() {
        assert!(validate_endpoint("x", &Url::parse("https://tools.example/run").unwrap()).is_ok());
        assert!(validate_endpoint("x", &Url::parse("http://127.0.0.1:9000/run").unwrap()).is_ok());
        assert!(validate_endpoint("x", &Url::parse("http://tools.example/run").unwrap()).is_err());
        assert!(
            validate_endpoint("x", &Url::parse("https://u:p@tools.example/run").unwrap()).is_err()
        );
        assert!(
            validate_endpoint("x", &Url::parse("https://tools.example/run?q=1").unwrap()).is_err()
        );
    }

    #[test]
    fn authenticated_roles_map_to_one_bounded_tool_role() {
        assert_eq!(role_from_authenticated_claims(&[]), ToolAccessRole::Guest);
        assert_eq!(
            role_from_authenticated_claims(&["aog:tool:admin".to_string()]),
            ToolAccessRole::Admin
        );
    }

    #[test]
    fn session_ids_are_bounded_before_authority_metadata() {
        assert!(validate_session_id("session-1:turn_2").is_ok());
        assert!(validate_session_id("").is_err());
        assert!(validate_session_id("../escape").is_err());
        assert!(validate_session_id(&"x".repeat(129)).is_err());
    }
}
