// AgentCgroup: Tool-call tracker
//
// Consumes structured JSON events from the agentcgroup eBPF binary
// and drives the cgroup manager to create/destroy per-tool-call cgroups.
//
// This is the bridge between BPF events and cgroup operations:
//   BPF ring buffer → JSON stdout → tracker → CgroupManager
//
// It integrates with the existing AgentSight runner/analyzer framework
// by implementing the Runner trait for event streaming.

use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_stream::Stream;
use futures::StreamExt;
use log::{debug, info, warn, error};
use async_trait::async_trait;

use crate::framework::core::Event;
use crate::framework::runners::{Runner, RunnerError, EventStream};
use crate::framework::runners::common::{BinaryExecutor, AnalyzerProcessor};
use crate::framework::analyzers::Analyzer;

use super::types::*;
use super::manager::CgroupManager;
use super::metrics::MetricsCollector;

// ---------------------------------------------------------------------------
// ToolCallTracker: Runner that spawns agentcgroup BPF binary
// ---------------------------------------------------------------------------

pub struct ToolCallTracker {
    binary_path: String,
    args: Vec<String>,
    analyzers: Vec<Box<dyn Analyzer>>,
    manager: Arc<Mutex<CgroupManager>>,
    metrics: Arc<MetricsCollector>,
}

impl ToolCallTracker {
    /// Create a tracker from a BinaryExtractor (embedded binary).
    pub fn new(
        binary_path: &str,
        manager: Arc<Mutex<CgroupManager>>,
        metrics: Arc<MetricsCollector>,
    ) -> Self {
        ToolCallTracker {
            binary_path: binary_path.to_string(),
            args: Vec::new(),
            analyzers: Vec::new(),
            manager,
            metrics,
        }
    }

    /// Set the target PID to track.
    pub fn target_pid(mut self, pid: u32) -> Self {
        self.args.extend(["-p".to_string(), pid.to_string()]);
        self
    }

    /// Set the target workload ID.
    pub fn workload_id(mut self, id: u32) -> Self {
        self.args.extend(["-w".to_string(), id.to_string()]);
        self
    }

    /// Set command filter.
    pub fn comm_filter(mut self, comm: &str) -> Self {
        self.args.extend(["-c".to_string(), comm.to_string()]);
        self
    }

    /// Set minimum duration filter (ms).
    pub fn min_duration(mut self, ms: u64) -> Self {
        self.args.extend(["-d".to_string(), ms.to_string()]);
        self
    }

    /// Enable verbose BPF output.
    pub fn verbose(mut self) -> Self {
        self.args.push("-v".to_string());
        self
    }
}

#[async_trait]
impl Runner for ToolCallTracker {
    async fn run(&mut self) -> Result<EventStream, RunnerError> {
        info!("Starting ToolCallTracker with binary: {}", self.binary_path);

        // Use BinaryExecutor to spawn the agentcgroup BPF binary
        let executor = BinaryExecutor::new(self.binary_path.clone())
            .with_args(&self.args)
            .with_runner_name("AgentCgroup".to_string());
        let json_stream = executor.get_json_stream().await?;

        // Transform raw JSON values into framework Events
        let manager = Arc::clone(&self.manager);
        let metrics = Arc::clone(&self.metrics);

        let event_stream = json_stream.map(|json_value| {
            let timestamp = json_value.get("timestamp_ns")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let pid = json_value.get("pid")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let comm = json_value.get("comm")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();

            Event::new_with_timestamp(
                timestamp,
                "agentcgroup".to_string(),
                pid,
                comm,
                json_value,
            )
        });

        // Apply cgroup management side effects on each event
        let mapped_stream = event_stream.then(move |event| {
            let manager = Arc::clone(&manager);
            let metrics = Arc::clone(&metrics);
            async move {
                if let Some(data) = event.data.as_object() {
                    let event_type = data
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    match event_type {
                        "tool_start" => {
                            handle_tool_start(&event, &manager, &metrics).await;
                        }
                        "tool_exit" => {
                            handle_tool_exit(&event, &manager, &metrics).await;
                        }
                        _ => {
                            debug!("Unhandled agentcgroup event type: {}", event_type);
                        }
                    }
                }
                event
            }
        });

        // Apply analyzer chain
        AnalyzerProcessor::process_through_analyzers(
            Box::pin(mapped_stream),
            &mut self.analyzers,
        ).await
    }

    fn add_analyzer(mut self, analyzer: Box<dyn Analyzer>) -> Self
    where
        Self: Sized,
    {
        self.analyzers.push(analyzer);
        self
    }

    fn name(&self) -> &str {
        "agentcgroup"
    }

    fn id(&self) -> String {
        "agentcgroup-tracker".to_string()
    }
}

// ---------------------------------------------------------------------------
// Event handlers
// ---------------------------------------------------------------------------

async fn handle_tool_start(
    event: &Event,
    manager: &Arc<Mutex<CgroupManager>>,
    metrics: &Arc<MetricsCollector>,
) {
    let data = match event.data.as_object() {
        Some(d) => d,
        None => return,
    };

    let pid = data.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let ppid = data.get("ppid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let workload_id = data.get("workload_id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let tool_type_str = data.get("tool_type").and_then(|v| v.as_str()).unwrap_or("unknown");
    let priority_str = data.get("priority").and_then(|v| v.as_str()).unwrap_or("medium");

    let tool_type = match tool_type_str {
        "bash" => ToolType::Bash,
        "test" => ToolType::Test,
        "build" => ToolType::Build,
        "install" => ToolType::Install,
        "git" => ToolType::Git,
        "editor" => ToolType::Editor,
        "python" => ToolType::Python,
        "node" => ToolType::Node,
        "lint" => ToolType::Lint,
        "format" => ToolType::Format,
        _ => ToolType::Unknown,
    };

    let priority = match priority_str {
        "low" => Priority::Low,
        "high" => Priority::High,
        _ => Priority::Medium,
    };

    // Create cgroup for this tool call
    let mut mgr = manager.lock().await;

    // If the workload doesn't exist yet, we can't create a tool-call cgroup.
    // The daemon should register workloads first.
    if workload_id == 0 || mgr.get_workload(workload_id).is_none() {
        debug!(
            "Tool call from unregistered workload {} (pid={}), skipping cgroup creation",
            workload_id, pid
        );
        return;
    }

    match mgr.create_tool_call(pid, ppid, workload_id, tool_type, priority) {
        Ok(tc) => {
            info!(
                "📦 Tool started: pid={} type={} priority={} cgroup={}",
                pid, tool_type, priority, tc.cgroup_path
            );
            metrics.record_tool_start(tool_type, priority, workload_id);
        }
        Err(e) => {
            warn!("Failed to create cgroup for tool call pid={}: {}", pid, e);
        }
    }
}

async fn handle_tool_exit(
    event: &Event,
    manager: &Arc<Mutex<CgroupManager>>,
    metrics: &Arc<MetricsCollector>,
) {
    let data = match event.data.as_object() {
        Some(d) => d,
        None => return,
    };

    let pid = data.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let duration_ns = data.get("duration_ns").and_then(|v| v.as_u64()).unwrap_or(0);
    let exit_code = data.get("exit_code").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let memory_bytes = data.get("memory_usage_bytes").and_then(|v| v.as_u64()).unwrap_or(0);
    let tool_type_str = data.get("tool_type").and_then(|v| v.as_str()).unwrap_or("unknown");
    let priority_str = data.get("priority").and_then(|v| v.as_str()).unwrap_or("medium");
    let workload_id = data.get("workload_id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

    let tool_type = match tool_type_str {
        "test" => ToolType::Test,
        "build" => ToolType::Build,
        "install" => ToolType::Install,
        "git" => ToolType::Git,
        "python" => ToolType::Python,
        "node" => ToolType::Node,
        "bash" => ToolType::Bash,
        _ => ToolType::Unknown,
    };

    let priority = match priority_str {
        "low" => Priority::Low,
        "high" => Priority::High,
        _ => Priority::Medium,
    };

    let duration_secs = duration_ns as f64 / 1_000_000_000.0;

    // Destroy the tool-call cgroup
    let mut mgr = manager.lock().await;
    match mgr.destroy_tool_call(pid) {
        Ok(()) => {
            info!(
                "✅ Tool exited: pid={} type={} duration={:.2}s exit_code={}",
                pid, tool_type, duration_secs, exit_code
            );
        }
        Err(e) => {
            debug!("Could not destroy cgroup for pid={}: {}", pid, e);
        }
    }

    metrics.record_tool_exit(tool_type, priority, workload_id, duration_ns, memory_bytes);
}

// ---------------------------------------------------------------------------
// CgroupAnalyzer: analyzes events for pressure monitoring
// Runs in the analyzer chain to check memory pressure on active tool calls
// and apply graceful degradation.
// ---------------------------------------------------------------------------

pub struct PressureMonitorAnalyzer {
    manager: Arc<Mutex<CgroupManager>>,
    metrics: Arc<MetricsCollector>,
}

impl PressureMonitorAnalyzer {
    pub fn new(
        manager: Arc<Mutex<CgroupManager>>,
        metrics: Arc<MetricsCollector>,
    ) -> Self {
        PressureMonitorAnalyzer { manager, metrics }
    }
}

#[async_trait]
impl Analyzer for PressureMonitorAnalyzer {
    async fn process(
        &mut self,
        stream: EventStream,
    ) -> Result<EventStream, crate::framework::analyzers::AnalyzerError> {
        let manager = Arc::clone(&self.manager);
        let metrics = Arc::clone(&self.metrics);

        let mapped = stream.then(move |event| {
            let manager = Arc::clone(&manager);
            let metrics = Arc::clone(&metrics);
            async move {
                // On each event, check pressure for all active tool calls
                let mgr = manager.lock().await;
                let pids: Vec<u32> = mgr.tool_calls().keys().copied().collect();
                drop(mgr);

                for pid in pids {
                    let mgr = manager.lock().await;
                    match mgr.check_pressure(pid) {
                        Ok((level, current, high)) => {
                            if level > DegradationLevel::Normal {
                                debug!(
                                    "Pressure on tool-{}: level={}, current={}MB, high={}MB",
                                    pid,
                                    level,
                                    current / (1024 * 1024),
                                    high / (1024 * 1024)
                                );
                            }
                            drop(mgr);

                            // Apply degradation if needed
                            if level > DegradationLevel::Normal {
                                let mut mgr = manager.lock().await;
                                if let Err(e) = mgr.apply_degradation(pid, level) {
                                    warn!("Failed to apply degradation to pid={}: {}", pid, e);
                                } else {
                                    metrics.record_throttle(level, 0);
                                    if level >= DegradationLevel::Freeze {
                                        metrics.record_freeze();
                                    }
                                }
                            }
                        }
                        Err(_) => {
                            // Tool call may have exited
                        }
                    }
                }

                event
            }
        });

        Ok(Box::pin(mapped))
    }

    fn name(&self) -> &str {
        "pressure_monitor"
    }
}
