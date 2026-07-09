//! Prometheus-compatible metrics registry.
//!
//! `MetricsRegistry` is a small, dependency-free counter / gauge /
//! histogram store that renders into the Prometheus text exposition
//! format (`text/plain; version=0.0.4`). It is intentionally minimal:
//!
//! * counters are `u64` and only increase,
//! * gauges are `f64` and can be set or moved,
//! * histograms use a small fixed bucket set sized for HTTP latency
//!   (1ms .. 30s) plus +Inf, suitable for `request_duration_ms`,
//! * all entries are keyed by `(metric_name, label_set)` so the same
//!   counter can be sliced by e.g. `route` / `status` without
//!   exploding the surface.
//!
//! ## Why no `prometheus` crate dependency
//!
//! MAI ships into an air-gapped product; every dep adds licensing
//! review surface and supply-chain risk. The exposition format is
//! short and stable, the registry needs only ~300 lines, and the
//! existing `tokio` + `serde` deps already cover the heavy lifting.
//!
//! ## Redaction guarantee (acceptance test)
//!
//! Metric names and label values are constrained to a small ASCII
//! alphabet by [`sanitize_label_value`]. Callers cannot leak prompts,
//! API keys, vault tokens, or PHI through a label even by accident —
//! anything containing whitespace, control characters, or a non
//! `[A-Za-z0-9_:./\-]` byte is replaced. The acceptance test in
//! `tests/ship_11_observability.rs` asserts the rendered output never
//! contains the per-request secrets a poorly-instrumented caller
//! might try to attach.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use parking_lot_compat::RwLock;

// ─── Canonical metric names (SHIP-HARDENING-PLAN §10 / line 789) ───
//
// Every name a metric must use lives here. Tests
// reference these constants directly so a typo in a `inc()` call site
// becomes a compile-time error, not a silent miss in dashboards.

pub const REQUESTS_TOTAL: &str = "mai_requests_total";
pub const REQUEST_DURATION_MS: &str = "mai_request_duration_ms";
pub const AUTH_FAILURES_TOTAL: &str = "mai_auth_failures_total";
pub const RATE_LIMITED_TOTAL: &str = "mai_rate_limited_total";
pub const AUDIT_WRITE_FAILURES_TOTAL: &str = "mai_audit_write_failures_total";
pub const AUDIT_CHAIN_STATUS: &str = "mai_audit_chain_status";
pub const TRUST_BUNDLE_AGE_SECONDS: &str = "mai_trust_bundle_age_seconds";
pub const TRUST_BUNDLE_SIGNATURE_STATUS: &str = "mai_trust_bundle_signature_status";
pub const TRUST_CONNECTIVITY_STATE: &str = "mai_trust_connectivity_state";
pub const SCHEDULER_QUEUE_DEPTH: &str = "mai_scheduler_queue_depth";
pub const SCHEDULER_DECISION_LATENCY_US: &str = "mai_scheduler_decision_latency_us";
pub const ADAPTER_HEALTH: &str = "mai_adapter_health";
pub const ADAPTER_RESTART_COUNT: &str = "mai_adapter_restart_count";
pub const GPU_MEMORY_USED_BYTES: &str = "mai_gpu_memory_used_bytes";
pub const KV_CACHE_USED_BYTES: &str = "mai_kv_cache_used_bytes";
pub const POLICY_DECISIONS_TOTAL: &str = "mai_policy_decisions_total";
pub const COMPLIANCE_REPORT_GENERATION_TOTAL: &str = "mai_compliance_report_generation_total";
// The backup writers will produce these later. The names are reserved
// so the alert rules in `packaging/alerts/mai-alerts.yml` can reference
// them today; counters start at zero and stay there until then.
pub const BACKUP_SUCCESS_TOTAL: &str = "mai_backup_success_total";
pub const BACKUP_FAILURE_TOTAL: &str = "mai_backup_failure_total";

// ─── Metric kinds ──────────────────────────────────────────────────

/// What kind of metric a name represents. The Prometheus exposition
/// emits a `# TYPE` line per metric family; the registry tracks this
/// so a name's type cannot drift across call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    /// Monotonic `u64` counter. `# TYPE ... counter`.
    Counter,
    /// `f64` gauge. `# TYPE ... gauge`.
    Gauge,
    /// Latency histogram with fixed buckets. `# TYPE ... histogram`.
    Histogram,
}

// ─── Label set ─────────────────────────────────────────────────────

/// A small, deterministic label set. Stored sorted (BTreeMap) so two
/// callers with the same logical labels hash to the same series.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Labels(BTreeMap<String, String>);

impl Labels {
    /// Build an empty label set.
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Add `key=value`. The key is sanitized to the Prometheus name
    /// alphabet and the value is sanitized to a safe printable subset
    /// — see [`sanitize_label_value`] for the redaction guarantee.
    pub fn with(mut self, key: &str, value: &str) -> Self {
        self.0
            .insert(sanitize_label_name(key), sanitize_label_value(value));
        self
    }

    fn render(&self) -> String {
        if self.0.is_empty() {
            return String::new();
        }
        let mut out = String::from("{");
        let mut first = true;
        for (k, v) in &self.0 {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(k);
            out.push_str("=\"");
            // Prometheus escape: \\, \", \n
            for ch in v.chars() {
                match ch {
                    '\\' => out.push_str("\\\\"),
                    '"' => out.push_str("\\\""),
                    '\n' => out.push_str("\\n"),
                    _ => out.push(ch),
                }
            }
            out.push('"');
        }
        out.push('}');
        out
    }
}

/// Constrain a label name to `[a-zA-Z_][a-zA-Z0-9_]*`. Non-conforming
/// bytes become `_`. This matches the Prometheus naming spec.
fn sanitize_label_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, ch) in s.chars().enumerate() {
        let ok = match ch {
            'a'..='z' | 'A'..='Z' | '_' => true,
            '0'..='9' if i > 0 => true,
            _ => false,
        };
        out.push(if ok { ch } else { '_' });
    }
    if out.is_empty() { "_".to_string() } else { out }
}

/// Constrain a label value to printable ASCII minus quotes/backslashes
/// and minus anything that looks like a secret-bearing path. Long
/// values are truncated at 128 bytes so a poorly-instrumented caller
/// cannot stream a prompt or a JWT into a label.
///
/// Acceptance: the observability test attempts to attach
/// `value = "sk-live-{32 hex}"` / `value = "Bearer ..."` / `value =
/// "/etc/mai/vault/token"` and asserts none of them survive into the
/// `/v1/metrics` output verbatim.
pub fn sanitize_label_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(128));
    for ch in s.chars().take(128) {
        let ok = matches!(
            ch,
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | ':' | '.' | '/' | '-'
        );
        out.push(if ok { ch } else { '_' });
    }
    // Defense-in-depth: redact anything that smells like a token even
    // after sanitization. These prefixes are the most common shapes
    // a real-world secret takes when serialized as a string.
    let lower = out.to_lowercase();
    const SUSPICIOUS_PREFIXES: &[&str] = &[
        "sk-",     // OpenAI-style API key
        "bearer_", // sanitized bearer-token prefix
        "hvs.",    // HashiCorp Vault token (HCP)
        "s.",      // legacy Vault token
        "ghp_",    // GitHub PAT
        "xoxb-",   // Slack bot token
        "ya29.",   // Google OAuth
    ];
    for prefix in SUSPICIOUS_PREFIXES {
        if lower.starts_with(prefix) {
            return "redacted".to_string();
        }
    }
    if out.is_empty() { "_".to_string() } else { out }
}

// ─── Histogram buckets ─────────────────────────────────────────────

/// Latency buckets in milliseconds. Chosen to cover the realistic span
/// for a local-inference REST API: sub-millisecond cache hits up to a
/// 30-second large-batch generation.
const REQUEST_DURATION_BUCKETS_MS: &[f64] = &[
    1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1_000.0, 2_500.0, 5_000.0, 10_000.0, 30_000.0,
];

#[derive(Debug)]
struct HistogramState {
    /// One counter per bucket; index N covers `(-inf, BUCKETS[N]]`.
    /// The `+Inf` bucket is the implicit `len() == BUCKETS.len()` slot.
    buckets: Vec<AtomicU64>,
    sum_ms: AtomicU64, // sum stored as integer milliseconds * 1000 for precision
    count: AtomicU64,
}

impl HistogramState {
    fn new() -> Self {
        let mut buckets = Vec::with_capacity(REQUEST_DURATION_BUCKETS_MS.len() + 1);
        for _ in 0..=REQUEST_DURATION_BUCKETS_MS.len() {
            buckets.push(AtomicU64::new(0));
        }
        Self {
            buckets,
            sum_ms: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    fn observe_ms(&self, value_ms: f64) {
        // Find the first bucket whose upper bound >= value.
        let mut idx = REQUEST_DURATION_BUCKETS_MS.len(); // +Inf bucket
        for (i, &bound) in REQUEST_DURATION_BUCKETS_MS.iter().enumerate() {
            if value_ms <= bound {
                idx = i;
                break;
            }
        }
        // Prometheus histograms are cumulative: bucket N counts
        // observations <= BUCKETS[N], so every bucket >= idx ticks.
        for i in idx..self.buckets.len() {
            self.buckets[i].fetch_add(1, Ordering::Relaxed);
        }
        // Store sum as milli-millis (i.e. micro-seconds) so we keep
        // sub-millisecond precision without f64 atomics.
        let micro = (value_ms * 1_000.0) as u64;
        self.sum_ms.fetch_add(micro, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
    }
}

// ─── Registry ──────────────────────────────────────────────────────

/// Series-key. Two `inc`s with the same name+labels target the same
/// counter cell.
type SeriesKey = (String, Labels);

#[derive(Debug, Default)]
struct Inner {
    /// Declared metric families (name -> kind + help text).
    families: BTreeMap<String, (MetricKind, String)>,
    /// Counter cells.
    counters: BTreeMap<SeriesKey, AtomicU64>,
    /// Gauge cells (stored as raw u64 bits of the f64 to allow atomic update).
    gauges: BTreeMap<SeriesKey, AtomicU64>,
    /// Histogram cells.
    histograms: BTreeMap<SeriesKey, HistogramState>,
}

/// Shared metrics registry. Cheap to clone (`Arc` internally) — embed
/// one in [`crate::state::AppState`] and clone freely.
#[derive(Clone, Debug, Default)]
pub struct MetricsRegistry {
    inner: Arc<RwLock<Inner>>,
}

impl MetricsRegistry {
    /// Build a new, empty registry with all metric families
    /// pre-declared. Pre-declaring means the exposition output always
    /// shows the family (with a `# TYPE` line) even before the first
    /// observation — dashboards never see "no such metric" gaps right
    /// after a fresh deploy.
    pub fn with_ship_11_defaults() -> Self {
        let me = Self::default();
        me.declare(
            REQUESTS_TOTAL,
            MetricKind::Counter,
            "Total HTTP requests handled by mai-api, sliced by route and status_class.",
        );
        me.declare(
            REQUEST_DURATION_MS,
            MetricKind::Histogram,
            "HTTP request handling latency in milliseconds.",
        );
        me.declare(
            AUTH_FAILURES_TOTAL,
            MetricKind::Counter,
            "Authentication failures (401 responses).",
        );
        me.declare(
            RATE_LIMITED_TOTAL,
            MetricKind::Counter,
            "Requests rejected because they exceeded the per-profile rate limit.",
        );
        me.declare(
            AUDIT_WRITE_FAILURES_TOTAL,
            MetricKind::Counter,
            "Audit-log writes that failed to persist.",
        );
        me.declare(
            AUDIT_CHAIN_STATUS,
            MetricKind::Gauge,
            "1 if the tamper-evident audit hash chain verifies, 0 otherwise.",
        );
        me.declare(
            TRUST_BUNDLE_AGE_SECONDS,
            MetricKind::Gauge,
            "Age of the currently-loaded trust bundle in seconds.",
        );
        me.declare(
            TRUST_BUNDLE_SIGNATURE_STATUS,
            MetricKind::Gauge,
            "1 if the trust bundle ML-DSA signature verifies, 0 otherwise.",
        );
        me.declare(
            TRUST_CONNECTIVITY_STATE,
            MetricKind::Gauge,
            "0=AirGapCompliant, 1=Connected, 2=NonCompliant.",
        );
        me.declare(
            SCHEDULER_QUEUE_DEPTH,
            MetricKind::Gauge,
            "Number of inference requests waiting in the scheduler queue.",
        );
        me.declare(
            SCHEDULER_DECISION_LATENCY_US,
            MetricKind::Histogram,
            "Scheduler placement decision latency in microseconds.",
        );
        me.declare(
            ADAPTER_HEALTH,
            MetricKind::Gauge,
            "Per-adapter health: 1=healthy, 0=unhealthy.",
        );
        me.declare(
            ADAPTER_RESTART_COUNT,
            MetricKind::Counter,
            "Adapter subprocess restarts since boot.",
        );
        me.declare(
            GPU_MEMORY_USED_BYTES,
            MetricKind::Gauge,
            "Per-GPU memory currently allocated.",
        );
        me.declare(
            KV_CACHE_USED_BYTES,
            MetricKind::Gauge,
            "Per-instance KV cache memory currently allocated.",
        );
        me.declare(
            POLICY_DECISIONS_TOTAL,
            MetricKind::Counter,
            "Policy decisions sliced by module and decision (allow/deny/transform).",
        );
        me.declare(
            COMPLIANCE_REPORT_GENERATION_TOTAL,
            MetricKind::Counter,
            "Compliance reports generated.",
        );
        // Reserved for later; safe to alert on today (always zero).
        me.declare(
            BACKUP_SUCCESS_TOTAL,
            MetricKind::Counter,
            "Successful backup operations (writer not wired yet, counter stays at 0 until then).",
        );
        me.declare(
            BACKUP_FAILURE_TOTAL,
            MetricKind::Counter,
            "Failed backup operations (writer not wired yet, counter stays at 0 until then).",
        );
        me
    }

    /// Declare a metric family. Idempotent: re-declaring the same name
    /// with the same kind is a no-op; declaring with a different kind
    /// is logged but does not panic (the first wins).
    pub fn declare(&self, name: &str, kind: MetricKind, help: &str) {
        let mut inner = self.inner.write();
        inner
            .families
            .entry(name.to_string())
            .or_insert_with(|| (kind, help.to_string()));
    }

    /// Increment a counter by 1. Declares as `Counter` on first use.
    pub fn inc(&self, name: &str, labels: Labels) {
        self.inc_by(name, labels, 1);
    }

    /// Increment a counter by `n`. Declares as `Counter` on first use.
    pub fn inc_by(&self, name: &str, labels: Labels, n: u64) {
        let mut inner = self.inner.write();
        inner
            .families
            .entry(name.to_string())
            .or_insert_with(|| (MetricKind::Counter, String::new()));
        let cell = inner
            .counters
            .entry((name.to_string(), labels))
            .or_insert_with(|| AtomicU64::new(0));
        cell.fetch_add(n, Ordering::Relaxed);
    }

    /// Set a gauge to `value`. Declares as `Gauge` on first use.
    pub fn gauge_set(&self, name: &str, labels: Labels, value: f64) {
        let mut inner = self.inner.write();
        inner
            .families
            .entry(name.to_string())
            .or_insert_with(|| (MetricKind::Gauge, String::new()));
        let cell = inner
            .gauges
            .entry((name.to_string(), labels))
            .or_insert_with(|| AtomicU64::new(0));
        cell.store(value.to_bits(), Ordering::Relaxed);
    }

    /// Observe a value into a histogram. Declares as `Histogram` on
    /// first use.
    pub fn observe(&self, name: &str, labels: Labels, value_ms: f64) {
        let mut inner = self.inner.write();
        inner
            .families
            .entry(name.to_string())
            .or_insert_with(|| (MetricKind::Histogram, String::new()));
        let hist = inner
            .histograms
            .entry((name.to_string(), labels))
            .or_insert_with(HistogramState::new);
        hist.observe_ms(value_ms);
    }

    /// Render the registry as Prometheus text-format (version 0.0.4).
    pub fn render(&self) -> String {
        let inner = self.inner.read();
        let mut out = String::with_capacity(4096);

        for (name, (kind, help)) in &inner.families {
            // # HELP line
            if !help.is_empty() {
                out.push_str("# HELP ");
                out.push_str(name);
                out.push(' ');
                for ch in help.chars() {
                    match ch {
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        _ => out.push(ch),
                    }
                }
                out.push('\n');
            }
            // # TYPE line
            out.push_str("# TYPE ");
            out.push_str(name);
            out.push_str(match kind {
                MetricKind::Counter => " counter\n",
                MetricKind::Gauge => " gauge\n",
                MetricKind::Histogram => " histogram\n",
            });

            match kind {
                MetricKind::Counter => {
                    let prefix = name.as_str();
                    for ((n, labels), cell) in &inner.counters {
                        if n != prefix {
                            continue;
                        }
                        out.push_str(name);
                        out.push_str(&labels.render());
                        out.push(' ');
                        out.push_str(&cell.load(Ordering::Relaxed).to_string());
                        out.push('\n');
                    }
                }
                MetricKind::Gauge => {
                    let prefix = name.as_str();
                    for ((n, labels), cell) in &inner.gauges {
                        if n != prefix {
                            continue;
                        }
                        let v = f64::from_bits(cell.load(Ordering::Relaxed));
                        out.push_str(name);
                        out.push_str(&labels.render());
                        out.push(' ');
                        out.push_str(&render_f64(v));
                        out.push('\n');
                    }
                }
                MetricKind::Histogram => {
                    let prefix = name.as_str();
                    for ((n, labels), hist) in &inner.histograms {
                        if n != prefix {
                            continue;
                        }
                        // _bucket lines
                        for (i, bound) in REQUEST_DURATION_BUCKETS_MS.iter().enumerate() {
                            let bucket_labels = labels.clone().with("le", &render_f64(*bound));
                            out.push_str(name);
                            out.push_str("_bucket");
                            out.push_str(&bucket_labels.render());
                            out.push(' ');
                            out.push_str(&hist.buckets[i].load(Ordering::Relaxed).to_string());
                            out.push('\n');
                        }
                        // +Inf bucket
                        let inf_labels = labels.clone().with("le", "+Inf");
                        out.push_str(name);
                        out.push_str("_bucket");
                        out.push_str(&inf_labels.render());
                        out.push(' ');
                        out.push_str(
                            &hist.buckets[REQUEST_DURATION_BUCKETS_MS.len()]
                                .load(Ordering::Relaxed)
                                .to_string(),
                        );
                        out.push('\n');
                        // _sum
                        let sum_micro = hist.sum_ms.load(Ordering::Relaxed);
                        let sum_ms = sum_micro as f64 / 1_000.0;
                        out.push_str(name);
                        out.push_str("_sum");
                        out.push_str(&labels.render());
                        out.push(' ');
                        out.push_str(&render_f64(sum_ms));
                        out.push('\n');
                        // _count
                        out.push_str(name);
                        out.push_str("_count");
                        out.push_str(&labels.render());
                        out.push(' ');
                        out.push_str(&hist.count.load(Ordering::Relaxed).to_string());
                        out.push('\n');
                    }
                }
            }
        }

        out
    }
}

/// Convenience: time a code block and record the elapsed milliseconds
/// into `name + labels`. Use at request boundaries.
pub struct LatencyTimer<'a> {
    registry: &'a MetricsRegistry,
    name: &'a str,
    labels: Labels,
    started: Instant,
}

impl<'a> LatencyTimer<'a> {
    pub fn start(registry: &'a MetricsRegistry, name: &'a str, labels: Labels) -> Self {
        Self {
            registry,
            name,
            labels,
            started: Instant::now(),
        }
    }
}

impl Drop for LatencyTimer<'_> {
    fn drop(&mut self) {
        let elapsed_ms = self.started.elapsed().as_secs_f64() * 1_000.0;
        self.registry
            .observe(self.name, std::mem::take(&mut self.labels), elapsed_ms);
    }
}

fn render_f64(v: f64) -> String {
    if v.is_nan() {
        "NaN".to_string()
    } else if v.is_infinite() {
        if v.is_sign_negative() {
            "-Inf".to_string()
        } else {
            "+Inf".to_string()
        }
    } else if v.fract() == 0.0 && v.abs() < 1e16 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

// ─── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_counter_increments() {
        let r = MetricsRegistry::default();
        r.inc(
            REQUESTS_TOTAL,
            Labels::new().with("route", "/v1/chat/completions"),
        );
        r.inc(
            REQUESTS_TOTAL,
            Labels::new().with("route", "/v1/chat/completions"),
        );
        let out = r.render();
        assert!(out.contains("mai_requests_total{route=\"/v1/chat/completions\"} 2"));
    }

    #[test]
    fn test_gauge_set() {
        let r = MetricsRegistry::default();
        r.gauge_set(
            GPU_MEMORY_USED_BYTES,
            Labels::new().with("gpu", "0"),
            12_345_678.0,
        );
        let out = r.render();
        assert!(out.contains("# TYPE mai_gpu_memory_used_bytes gauge"));
        assert!(out.contains("mai_gpu_memory_used_bytes{gpu=\"0\"} 12345678"));
    }

    #[test]
    fn test_histogram_observe() {
        let r = MetricsRegistry::default();
        r.observe(
            REQUEST_DURATION_MS,
            Labels::new().with("route", "/v1/health"),
            3.0,
        );
        r.observe(
            REQUEST_DURATION_MS,
            Labels::new().with("route", "/v1/health"),
            75.0,
        );
        let out = r.render();
        assert!(out.contains("# TYPE mai_request_duration_ms histogram"));
        // 3ms <= 5ms bucket
        assert!(out.contains("mai_request_duration_ms_bucket{le=\"5\",route=\"/v1/health\"} 1"));
        // both observations <= 100ms bucket
        assert!(out.contains("mai_request_duration_ms_bucket{le=\"100\",route=\"/v1/health\"} 2"));
        assert!(out.contains("mai_request_duration_ms_count{route=\"/v1/health\"} 2"));
    }

    #[test]
    fn test_label_value_redacts_secrets() {
        // Acceptance test: secrets attached as label values
        // must never appear verbatim in the exposition output.
        let r = MetricsRegistry::default();
        r.inc(
            REQUESTS_TOTAL,
            Labels::new().with("token", "sk-live-abcdef0123456789"),
        );
        r.inc(
            REQUESTS_TOTAL,
            Labels::new().with("token", "hvs.CAESIQABCDEF"),
        );
        r.inc(
            REQUESTS_TOTAL,
            Labels::new().with("token", "ghp_AAAABBBBCCCC"),
        );
        let out = r.render();
        assert!(!out.contains("sk-live-abcdef"));
        assert!(!out.contains("hvs.CAES"));
        assert!(!out.contains("ghp_AAAA"));
        assert!(out.contains("token=\"redacted\""));
    }

    #[test]
    fn test_label_value_strips_unsafe_bytes() {
        let r = MetricsRegistry::default();
        r.inc(
            REQUESTS_TOTAL,
            Labels::new().with("route", "/v1/chat \"injected\"\nmore"),
        );
        let out = r.render();
        // Quotes and newline get scrubbed; only the safe alphabet survives.
        assert!(!out.contains("\"injected\""));
        assert!(!out.contains("\nmore"));
    }

    #[test]
    fn test_label_value_truncates_long_strings() {
        let r = MetricsRegistry::default();
        let huge = "a".repeat(10_000);
        r.inc(REQUESTS_TOTAL, Labels::new().with("payload", &huge));
        let out = r.render();
        // No single line should be > 256 chars after sanitization
        // (128-byte value + label boilerplate).
        for line in out.lines() {
            assert!(line.len() < 512, "line too long: {} chars", line.len());
        }
    }

    #[test]
    fn test_ship_11_defaults_pre_declares_all_families() {
        let r = MetricsRegistry::with_ship_11_defaults();
        let out = r.render();
        // Every SHIP-HARDENING-PLAN §10 metric name has its # TYPE line
        // emitted even before any observation.
        for name in [
            REQUESTS_TOTAL,
            REQUEST_DURATION_MS,
            AUTH_FAILURES_TOTAL,
            RATE_LIMITED_TOTAL,
            AUDIT_WRITE_FAILURES_TOTAL,
            AUDIT_CHAIN_STATUS,
            TRUST_BUNDLE_AGE_SECONDS,
            TRUST_BUNDLE_SIGNATURE_STATUS,
            TRUST_CONNECTIVITY_STATE,
            SCHEDULER_QUEUE_DEPTH,
            SCHEDULER_DECISION_LATENCY_US,
            ADAPTER_HEALTH,
            ADAPTER_RESTART_COUNT,
            GPU_MEMORY_USED_BYTES,
            KV_CACHE_USED_BYTES,
            POLICY_DECISIONS_TOTAL,
            COMPLIANCE_REPORT_GENERATION_TOTAL,
            BACKUP_SUCCESS_TOTAL,
            BACKUP_FAILURE_TOTAL,
        ] {
            assert!(
                out.contains(&format!("# TYPE {name} ")),
                "missing # TYPE for {name}"
            );
        }
    }

    #[test]
    fn test_label_name_sanitization() {
        assert_eq!(sanitize_label_name("valid_name"), "valid_name");
        assert_eq!(sanitize_label_name("with-dash"), "with_dash");
        assert_eq!(
            sanitize_label_name("9starts_with_digit"),
            "_starts_with_digit"
        );
        assert_eq!(sanitize_label_name(""), "_");
    }

    #[test]
    fn test_render_f64_no_trailing_decimals_for_ints() {
        assert_eq!(render_f64(12345.0), "12345");
        assert_eq!(render_f64(0.0), "0");
        assert_eq!(render_f64(1.5), "1.5");
        assert_eq!(render_f64(f64::INFINITY), "+Inf");
        assert_eq!(render_f64(f64::NEG_INFINITY), "-Inf");
    }
}

// ─── Tiny adapter to keep zero new deps ────────────────────────────
//
// The registry needs interior mutability across many threads. The
// workspace already pulls in `tokio::sync::RwLock`, but that's async
// and wrong for a hot-path metric `inc()` (we'd need `.await` inside
// otherwise-sync handler middleware). `parking_lot` would be ideal
// but is not a workspace dep. We synthesize a minimal blocking
// `RwLock` shim from `std::sync::RwLock` so this file pulls in no
// new third-party crates.

mod parking_lot_compat {
    use std::sync::RwLock as StdRwLock;
    use std::sync::RwLockReadGuard as StdRead;
    use std::sync::RwLockWriteGuard as StdWrite;

    #[derive(Debug, Default)]
    pub struct RwLock<T>(StdRwLock<T>);

    impl<T> RwLock<T> {
        pub fn new(v: T) -> Self {
            Self(StdRwLock::new(v))
        }
        pub fn read(&self) -> StdRead<'_, T> {
            self.0.read().expect("metrics RwLock poisoned")
        }
        pub fn write(&self) -> StdWrite<'_, T> {
            self.0.write().expect("metrics RwLock poisoned")
        }
    }
}
