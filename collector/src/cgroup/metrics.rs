// AgentCgroup: Prometheus metrics exporter
//
// Collects and exposes metrics in Prometheus exposition format via HTTP.
// Metrics tracked:
//   - agentcgroup_tool_calls_total{type, priority, workload}
//   - agentcgroup_tool_call_duration_seconds{type, priority}
//   - agentcgroup_memory_usage_bytes{workload, tool_pid}
//   - agentcgroup_throttle_events_total{workload, level}
//   - agentcgroup_freeze_events_total{workload}
//   - agentcgroup_oom_kills_total{workload}
//   - agentcgroup_active_tool_calls{workload}
//   - agentcgroup_degradation_level{workload, tool_pid}

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use serde::Serialize;

use super::types::*;

// ---------------------------------------------------------------------------
// MetricsCollector: thread-safe metrics aggregation
// ---------------------------------------------------------------------------

pub struct MetricsCollector {
    /// Total tool calls started per (tool_type, priority)
    tool_calls_started: DashCounter,
    /// Total tool calls completed
    tool_calls_exited: DashCounter,
    /// Total throttle events per degradation level
    throttle_events: DashCounter,
    /// Total freeze events
    freeze_events: AtomicU64,
    /// Total OOM kills
    oom_kills: AtomicU64,
    /// Current active tool calls per workload
    active_tool_calls: DashGauge,
    /// Tool-call duration histogram buckets (in seconds)
    duration_histogram: DashHistogram,
    /// Peak memory per tool call (bytes)
    peak_memory: DashGauge,
    /// Snapshot of last-known degradation levels
    degradation_levels: DashGauge,
}

impl MetricsCollector {
    pub fn new() -> Self {
        MetricsCollector {
            tool_calls_started: DashCounter::new(),
            tool_calls_exited: DashCounter::new(),
            throttle_events: DashCounter::new(),
            freeze_events: AtomicU64::new(0),
            oom_kills: AtomicU64::new(0),
            active_tool_calls: DashGauge::new(),
            duration_histogram: DashHistogram::new(),
            peak_memory: DashGauge::new(),
            degradation_levels: DashGauge::new(),
        }
    }

    /// Record a tool-call start event.
    pub fn record_tool_start(&self, tool_type: ToolType, priority: Priority, workload_id: u32) {
        let key = format!("{}_{}", tool_type, priority);
        self.tool_calls_started.inc(&key);

        let wl_key = format!("workload_{}", workload_id);
        self.active_tool_calls.inc(&wl_key);
    }

    /// Record a tool-call exit event with duration.
    pub fn record_tool_exit(
        &self,
        tool_type: ToolType,
        priority: Priority,
        workload_id: u32,
        duration_ns: u64,
        peak_memory_bytes: u64,
    ) {
        let key = format!("{}_{}", tool_type, priority);
        self.tool_calls_exited.inc(&key);

        let wl_key = format!("workload_{}", workload_id);
        self.active_tool_calls.dec(&wl_key);

        // Record duration in seconds
        let duration_secs = duration_ns as f64 / 1_000_000_000.0;
        self.duration_histogram.observe(&key, duration_secs);

        // Track peak memory
        let mem_key = format!("{}_{}", tool_type, workload_id);
        self.peak_memory.set(&mem_key, peak_memory_bytes);
    }

    /// Record a throttle event.
    pub fn record_throttle(&self, level: DegradationLevel, workload_id: u32) {
        let key = format!("{}_{}", level, workload_id);
        self.throttle_events.inc(&key);
    }

    /// Record a freeze event.
    pub fn record_freeze(&self) {
        self.freeze_events.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an OOM kill.
    pub fn record_oom(&self) {
        self.oom_kills.fetch_add(1, Ordering::Relaxed);
    }

    /// Update degradation level gauge for a tool call.
    pub fn set_degradation(&self, pid: u32, workload_id: u32, level: DegradationLevel) {
        let key = format!("{}_{}", pid, workload_id);
        self.degradation_levels.set(&key, level as u64);
    }

    /// Render all metrics in Prometheus exposition format.
    pub fn render_prometheus(&self) -> String {
        let mut output = String::with_capacity(4096);

        // Tool calls started
        output.push_str("# HELP agentcgroup_tool_calls_started_total Total tool calls started\n");
        output.push_str("# TYPE agentcgroup_tool_calls_started_total counter\n");
        for (key, value) in self.tool_calls_started.snapshot() {
            let parts: Vec<&str> = key.splitn(2, '_').collect();
            if parts.len() == 2 {
                output.push_str(&format!(
                    "agentcgroup_tool_calls_started_total{{tool_type=\"{}\",priority=\"{}\"}} {}\n",
                    parts[0], parts[1], value
                ));
            }
        }

        // Tool calls exited
        output.push_str("# HELP agentcgroup_tool_calls_exited_total Total tool calls completed\n");
        output.push_str("# TYPE agentcgroup_tool_calls_exited_total counter\n");
        for (key, value) in self.tool_calls_exited.snapshot() {
            let parts: Vec<&str> = key.splitn(2, '_').collect();
            if parts.len() == 2 {
                output.push_str(&format!(
                    "agentcgroup_tool_calls_exited_total{{tool_type=\"{}\",priority=\"{}\"}} {}\n",
                    parts[0], parts[1], value
                ));
            }
        }

        // Active tool calls
        output.push_str("# HELP agentcgroup_active_tool_calls Currently active tool calls\n");
        output.push_str("# TYPE agentcgroup_active_tool_calls gauge\n");
        for (key, value) in self.active_tool_calls.snapshot() {
            output.push_str(&format!(
                "agentcgroup_active_tool_calls{{{}}} {}\n",
                key, value
            ));
        }

        // Throttle events
        output.push_str("# HELP agentcgroup_throttle_events_total Total throttle events\n");
        output.push_str("# TYPE agentcgroup_throttle_events_total counter\n");
        for (key, value) in self.throttle_events.snapshot() {
            let parts: Vec<&str> = key.splitn(2, '_').collect();
            if parts.len() == 2 {
                output.push_str(&format!(
                    "agentcgroup_throttle_events_total{{level=\"{}\",workload=\"{}\"}} {}\n",
                    parts[0], parts[1], value
                ));
            }
        }

        // Freeze events
        output.push_str("# HELP agentcgroup_freeze_events_total Total freeze events\n");
        output.push_str("# TYPE agentcgroup_freeze_events_total counter\n");
        output.push_str(&format!(
            "agentcgroup_freeze_events_total {}\n",
            self.freeze_events.load(Ordering::Relaxed)
        ));

        // OOM kills
        output.push_str("# HELP agentcgroup_oom_kills_total Total OOM kills\n");
        output.push_str("# TYPE agentcgroup_oom_kills_total counter\n");
        output.push_str(&format!(
            "agentcgroup_oom_kills_total {}\n",
            self.oom_kills.load(Ordering::Relaxed)
        ));

        // Duration histogram
        output.push_str("# HELP agentcgroup_tool_call_duration_seconds Tool call duration distribution\n");
        output.push_str("# TYPE agentcgroup_tool_call_duration_seconds histogram\n");
        for (key, hist) in self.duration_histogram.snapshot() {
            let parts: Vec<&str> = key.splitn(2, '_').collect();
            if parts.len() == 2 {
                let labels = format!("tool_type=\"{}\",priority=\"{}\"", parts[0], parts[1]);
                for (bucket, count) in &hist.buckets {
                    output.push_str(&format!(
                        "agentcgroup_tool_call_duration_seconds_bucket{{{},le=\"{}\"}} {}\n",
                        labels, bucket, count
                    ));
                }
                output.push_str(&format!(
                    "agentcgroup_tool_call_duration_seconds_bucket{{{},le=\"+Inf\"}} {}\n",
                    labels, hist.count
                ));
                output.push_str(&format!(
                    "agentcgroup_tool_call_duration_seconds_sum{{{}}} {:.6}\n",
                    labels, hist.sum
                ));
                output.push_str(&format!(
                    "agentcgroup_tool_call_duration_seconds_count{{{}}} {}\n",
                    labels, hist.count
                ));
            }
        }

        output
    }

    /// Return a JSON snapshot of metrics for the web API.
    pub fn snapshot_json(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            tool_calls_started: self.tool_calls_started.total(),
            tool_calls_exited: self.tool_calls_exited.total(),
            freeze_events: self.freeze_events.load(Ordering::Relaxed),
            oom_kills: self.oom_kills.load(Ordering::Relaxed),
            active_tool_calls: self.active_tool_calls.snapshot(),
        }
    }
}

// ---------------------------------------------------------------------------
// Metrics snapshot for JSON serialization
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct MetricsSnapshot {
    pub tool_calls_started: u64,
    pub tool_calls_exited: u64,
    pub freeze_events: u64,
    pub oom_kills: u64,
    pub active_tool_calls: Vec<(String, u64)>,
}

// ---------------------------------------------------------------------------
// Internal: Lock-free counter map
// Uses a simple HashMap behind a mutex for simplicity. In production,
// consider using dashmap or per-CPU sharding.
// ---------------------------------------------------------------------------

struct DashCounter {
    inner: std::sync::Mutex<HashMap<String, u64>>,
}

impl DashCounter {
    fn new() -> Self {
        DashCounter {
            inner: std::sync::Mutex::new(HashMap::new()),
        }
    }

    fn inc(&self, key: &str) {
        let mut map = self.inner.lock().unwrap();
        *map.entry(key.to_string()).or_insert(0) += 1;
    }

    fn snapshot(&self) -> Vec<(String, u64)> {
        let map = self.inner.lock().unwrap();
        map.iter().map(|(k, v)| (k.clone(), *v)).collect()
    }

    fn total(&self) -> u64 {
        let map = self.inner.lock().unwrap();
        map.values().sum()
    }
}

// ---------------------------------------------------------------------------
// Internal: Lock-free gauge map (can increase and decrease)
// ---------------------------------------------------------------------------

struct DashGauge {
    inner: std::sync::Mutex<HashMap<String, u64>>,
}

impl DashGauge {
    fn new() -> Self {
        DashGauge {
            inner: std::sync::Mutex::new(HashMap::new()),
        }
    }

    fn inc(&self, key: &str) {
        let mut map = self.inner.lock().unwrap();
        *map.entry(key.to_string()).or_insert(0) += 1;
    }

    fn dec(&self, key: &str) {
        let mut map = self.inner.lock().unwrap();
        let val = map.entry(key.to_string()).or_insert(0);
        *val = val.saturating_sub(1);
    }

    fn set(&self, key: &str, value: u64) {
        let mut map = self.inner.lock().unwrap();
        map.insert(key.to_string(), value);
    }

    fn snapshot(&self) -> Vec<(String, u64)> {
        let map = self.inner.lock().unwrap();
        map.iter().map(|(k, v)| (k.clone(), *v)).collect()
    }
}

// ---------------------------------------------------------------------------
// Internal: Simple histogram with fixed buckets
// ---------------------------------------------------------------------------

struct DashHistogram {
    inner: std::sync::Mutex<HashMap<String, HistogramData>>,
}

#[derive(Debug, Clone)]
struct HistogramData {
    buckets: Vec<(f64, u64)>, // (le, count)
    sum: f64,
    count: u64,
}

impl HistogramData {
    fn new() -> Self {
        // Buckets: 0.1s, 0.5s, 1s, 2s, 5s, 10s, 30s, 60s, 120s, 300s
        let bucket_bounds = vec![0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0];
        HistogramData {
            buckets: bucket_bounds.into_iter().map(|b| (b, 0)).collect(),
            sum: 0.0,
            count: 0,
        }
    }

    fn observe(&mut self, value: f64) {
        self.sum += value;
        self.count += 1;
        for (le, count) in &mut self.buckets {
            if value <= *le {
                *count += 1;
            }
        }
    }
}

impl DashHistogram {
    fn new() -> Self {
        DashHistogram {
            inner: std::sync::Mutex::new(HashMap::new()),
        }
    }

    fn observe(&self, key: &str, value: f64) {
        let mut map = self.inner.lock().unwrap();
        map.entry(key.to_string())
            .or_insert_with(HistogramData::new)
            .observe(value);
    }

    fn snapshot(&self) -> Vec<(String, HistogramData)> {
        let map = self.inner.lock().unwrap();
        map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

// ---------------------------------------------------------------------------
// Metrics HTTP server (serves Prometheus /metrics endpoint)
// ---------------------------------------------------------------------------

pub struct MetricsServer {
    metrics: Arc<MetricsCollector>,
    port: u16,
}

impl MetricsServer {
    pub fn new(metrics: Arc<MetricsCollector>, port: u16) -> Self {
        MetricsServer { metrics, port }
    }

    /// Start the metrics HTTP server (spawns a tokio task).
    pub async fn start(&self) -> Result<tokio::task::JoinHandle<()>, Box<dyn std::error::Error>> {
        let metrics = Arc::clone(&self.metrics);
        let port = self.port;

        let handle = tokio::spawn(async move {
            use hyper::service::service_fn;
            use hyper::{Request, Response, body::Bytes};
            use hyper_util::rt::TokioIo;
            use http_body_util::Full;

            let addr: std::net::SocketAddr = ([0, 0, 0, 0], port).into();
            let listener = match tokio::net::TcpListener::bind(addr).await {
                Ok(l) => l,
                Err(e) => {
                    log::error!("Failed to bind metrics server on port {}: {}", port, e);
                    return;
                }
            };

            log::info!("Metrics server listening on http://0.0.0.0:{}/metrics", port);

            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(e) => {
                        log::warn!("Failed to accept connection: {}", e);
                        continue;
                    }
                };

                let metrics = Arc::clone(&metrics);
                let io = TokioIo::new(stream);

                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<hyper::body::Incoming>| {
                        let metrics = Arc::clone(&metrics);
                        async move {
                            match req.uri().path() {
                                "/metrics" => {
                                    let body = metrics.render_prometheus();
                                    Ok::<_, hyper::Error>(
                                        Response::builder()
                                            .header("Content-Type", "text/plain; version=0.0.4; charset=utf-8")
                                            .body(Full::new(Bytes::from(body)))
                                            .unwrap()
                                    )
                                }
                                "/api/metrics" => {
                                    let snap = metrics.snapshot_json();
                                    let body = serde_json::to_string(&snap).unwrap_or_default();
                                    Ok(Response::builder()
                                        .header("Content-Type", "application/json")
                                        .body(Full::new(Bytes::from(body)))
                                        .unwrap())
                                }
                                _ => {
                                    Ok(Response::builder()
                                        .status(404)
                                        .body(Full::new(Bytes::from("Not Found")))
                                        .unwrap())
                                }
                            }
                        }
                    });

                    if let Err(e) = hyper_util::server::conn::auto::Builder::new(
                        hyper_util::rt::TokioExecutor::new()
                    )
                    .serve_connection(io, service)
                    .await
                    {
                        log::debug!("Metrics connection error: {}", e);
                    }
                });
            }
        });

        Ok(handle)
    }
}
