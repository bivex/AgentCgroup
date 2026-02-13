// AgentCgroup: Main daemon
//
// The daemon is the top-level orchestrator that:
//   1. Loads configuration (YAML or defaults)
//   2. Initializes the cgroup v2 hierarchy
//   3. Spawns the agentcgroup eBPF binary
//   4. Processes tool-call events and manages cgroups
//   5. Runs the pressure monitoring loop
//   6. Exports Prometheus metrics
//
// It ties together: PolicyEngine + CgroupManager + ToolCallTracker + Metrics

use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};
use log::{info, warn, error};
use futures::StreamExt;

use super::types::*;
use super::manager::CgroupManager;
use super::policy::{AgentCgroupConfig, PolicyEngine, WorkloadPolicy};
use super::metrics::{MetricsCollector, MetricsServer};

// ---------------------------------------------------------------------------
// DaemonConfig: runtime parameters for the daemon
// ---------------------------------------------------------------------------

pub struct DaemonConfig {
    /// Path to the agentcgroup eBPF binary
    pub binary_path: String,
    /// Path to YAML config file (optional)
    pub config_path: Option<String>,
    /// Target PID to track (optional, overrides config)
    pub target_pid: Option<u32>,
    /// Command filter (optional)
    pub comm_filter: Option<String>,
    /// Workload ID to assign (optional)
    pub workload_id: Option<u32>,
    /// Metrics port
    pub metrics_port: u16,
    /// Enable auto-degradation
    pub auto_degrade: bool,
    /// Pressure monitoring interval (ms)
    pub monitor_interval_ms: u64,
    /// Dry-run mode (don't actually create cgroups)
    pub dry_run: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        DaemonConfig {
            binary_path: String::new(),
            config_path: None,
            target_pid: None,
            comm_filter: None,
            workload_id: None,
            metrics_port: 9090,
            auto_degrade: true,
            monitor_interval_ms: 500,
            dry_run: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Daemon: the main AgentCgroup service
// ---------------------------------------------------------------------------

pub struct Daemon {
    config: DaemonConfig,
    policy: PolicyEngine,
    manager: Arc<Mutex<CgroupManager>>,
    metrics: Arc<MetricsCollector>,
}

impl Daemon {
    /// Create a new daemon from configuration.
    pub fn new(config: DaemonConfig) -> Result<Self, Box<dyn std::error::Error>> {
        // Load policy config
        let policy_config = if let Some(ref path) = config.config_path {
            AgentCgroupConfig::from_file(Path::new(path))
                .map_err(|e| format!("Failed to load config: {}", e))?
        } else if let Some(pid) = config.target_pid {
            let comm = config.comm_filter.as_deref().unwrap_or("agent");
            AgentCgroupConfig::default_for_pid(pid, comm)
        } else {
            AgentCgroupConfig::default()
        };

        let policy = PolicyEngine::new(policy_config);
        let manager = Arc::new(Mutex::new(
            CgroupManager::new().map_err(|e| format!("Failed to create CgroupManager: {}", e))?,
        ));
        let metrics = Arc::new(MetricsCollector::new());

        Ok(Daemon {
            config,
            policy,
            manager,
            metrics,
        })
    }

    /// Run the daemon main loop.
    pub async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        info!("Starting AgentCgroup daemon");

        // Step 1: Initialize cgroup hierarchy
        if !self.config.dry_run {
            let mgr = self.manager.lock().await;
            if let Err(e) = mgr.init() {
                warn!("Could not initialize cgroup hierarchy (running without root?): {}", e);
                warn!("Continuing in monitor-only mode...");
            }
        }

        // Step 2: Register initial workload if target PID is specified
        if let Some(pid) = self.config.target_pid {
            let wid = self.config.workload_id.unwrap_or(1);
            let comm = self.config.comm_filter.as_deref().unwrap_or("agent");

            if let Some(wl_policy) = self.policy.default_workload() {
                if !self.config.dry_run {
                    let mut mgr = self.manager.lock().await;
                    match mgr.create_workload(wid, pid, comm, wl_policy) {
                        Ok(wl) => {
                            info!(
                                "Registered workload-{} for pid={} (agent={}, baseline={}MB)",
                                wid, pid, comm, wl_policy.baseline_memory_mb
                            );
                        }
                        Err(e) => {
                            warn!("Failed to create workload cgroup: {}", e);
                        }
                    }
                }
            }
        }

        // Step 3: Start metrics server
        let metrics_server = MetricsServer::new(
            Arc::clone(&self.metrics),
            self.config.metrics_port,
        );
        let _metrics_handle = metrics_server.start().await?;
        info!("📊 Metrics available at http://0.0.0.0:{}/metrics", self.config.metrics_port);

        // Step 4: Start pressure monitoring loop
        if self.config.auto_degrade && !self.config.dry_run {
            let manager = Arc::clone(&self.manager);
            let metrics = Arc::clone(&self.metrics);
            let interval_ms = self.config.monitor_interval_ms;

            tokio::spawn(async move {
                pressure_monitor_loop(manager, metrics, interval_ms).await;
            });
            info!("🔍 Pressure monitoring enabled (interval={}ms)", self.config.monitor_interval_ms);
        }

        // Step 5: Spawn agentcgroup BPF binary and process events
        if !self.config.binary_path.is_empty() {
            info!("🔧 Starting BPF tool-call tracker: {}", self.config.binary_path);
            self.run_tracker().await?;
        } else {
            info!("⚠️  No BPF binary path specified, running in passive mode");
            info!("   Use --binary-path to specify the agentcgroup binary");
            // In passive mode, just keep running for metrics
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }

        Ok(())
    }

    /// Run the eBPF tool-call tracker and process its events.
    async fn run_tracker(&self) -> Result<(), Box<dyn std::error::Error>> {
        use crate::framework::runners::common::BinaryExecutor;
        use crate::framework::core::Event;

        let mut args = Vec::new();
        if let Some(pid) = self.config.target_pid {
            args.extend(["-p".to_string(), pid.to_string()]);
        }
        if let Some(ref comm) = self.config.comm_filter {
            args.extend(["-c".to_string(), comm.clone()]);
        }
        if let Some(wid) = self.config.workload_id {
            args.extend(["-w".to_string(), wid.to_string()]);
        }

        let executor = BinaryExecutor::new(self.config.binary_path.clone())
            .with_args(&args)
            .with_runner_name("AgentCgroup".to_string());
        let json_stream = executor.get_json_stream().await.map_err(|e| {
            format!("Failed to start agentcgroup binary: {}", e)
        })?;

        // Convert JSON stream to Event stream
        let mut stream = json_stream.map(|json_value| {
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
            Event::new_with_timestamp(timestamp, "agentcgroup".to_string(), pid, comm, json_value)
        });

        info!("Tool-call event stream started, processing events...");

        while let Some(event) = stream.next().await {
            // Parse the tool-call event from the framework Event
            if let Some(data) = event.data.as_object() {
                let event_type = data
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                match event_type {
                    "tool_start" => {
                        self.handle_tool_start(data).await;
                    }
                    "tool_exit" => {
                        self.handle_tool_exit(data).await;
                    }
                    _ => {}
                }
            }
        }

        Ok(())
    }

    async fn handle_tool_start(&self, data: &serde_json::Map<String, serde_json::Value>) {
        let pid = data.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let ppid = data.get("ppid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let workload_id = data.get("workload_id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let tool_str = data.get("tool_type").and_then(|v| v.as_str()).unwrap_or("unknown");
        let prio_str = data.get("priority").and_then(|v| v.as_str()).unwrap_or("medium");

        let tool_type = parse_tool_type(tool_str);
        let priority = parse_priority(prio_str);

        if workload_id == 0 {
            return;
        }

        if !self.config.dry_run {
            let mut mgr = self.manager.lock().await;
            if mgr.get_workload(workload_id).is_some() {
                match mgr.create_tool_call(pid, ppid, workload_id, tool_type, priority) {
                    Ok(tc) => {
                        info!(
                            "📦 Tool started: pid={} type={} priority={} mem_high={}MB",
                            pid, tool_type, priority, tc.memory_high_bytes / (1024 * 1024)
                        );
                    }
                    Err(e) => {
                        warn!("Failed to create cgroup for pid={}: {}", pid, e);
                    }
                }
            }
        }

        self.metrics.record_tool_start(tool_type, priority, workload_id);
    }

    async fn handle_tool_exit(&self, data: &serde_json::Map<String, serde_json::Value>) {
        let pid = data.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let duration_ns = data.get("duration_ns").and_then(|v| v.as_u64()).unwrap_or(0);
        let exit_code = data.get("exit_code").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let memory_bytes = data.get("memory_usage_bytes").and_then(|v| v.as_u64()).unwrap_or(0);
        let tool_str = data.get("tool_type").and_then(|v| v.as_str()).unwrap_or("unknown");
        let prio_str = data.get("priority").and_then(|v| v.as_str()).unwrap_or("medium");
        let workload_id = data.get("workload_id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

        let tool_type = parse_tool_type(tool_str);
        let priority = parse_priority(prio_str);
        let duration_secs = duration_ns as f64 / 1_000_000_000.0;

        if !self.config.dry_run {
            let mut mgr = self.manager.lock().await;
            if let Err(e) = mgr.destroy_tool_call(pid) {
                // This is normal if the cgroup was already cleaned up
                log::debug!("Could not destroy cgroup for pid={}: {}", pid, e);
            }
        }

        info!(
            "✅ Tool exited: pid={} type={} duration={:.2}s exit={}",
            pid, tool_type, duration_secs, exit_code
        );

        self.metrics
            .record_tool_exit(tool_type, priority, workload_id, duration_ns, memory_bytes);
    }

    /// Graceful shutdown: clean up all cgroups.
    pub async fn shutdown(&mut self) {
        info!("Shutting down AgentCgroup daemon...");
        if !self.config.dry_run {
            let mut mgr = self.manager.lock().await;
            if let Err(e) = mgr.cleanup() {
                warn!("Error during cgroup cleanup: {}", e);
            }
        }
        info!("AgentCgroup daemon stopped");
    }

    pub fn metrics(&self) -> &Arc<MetricsCollector> {
        &self.metrics
    }

    pub fn manager(&self) -> &Arc<Mutex<CgroupManager>> {
        &self.manager
    }
}

// ---------------------------------------------------------------------------
// Pressure monitoring background task
// ---------------------------------------------------------------------------

async fn pressure_monitor_loop(
    manager: Arc<Mutex<CgroupManager>>,
    metrics: Arc<MetricsCollector>,
    interval_ms: u64,
) {
    let mut timer = interval(Duration::from_millis(interval_ms));

    loop {
        timer.tick().await;

        let mgr = manager.lock().await;
        let pids: Vec<u32> = mgr.tool_calls().keys().copied().collect();
        drop(mgr);

        for pid in pids {
            let mgr = manager.lock().await;
            let pressure_result = mgr.check_pressure(pid);
            drop(mgr);

            match pressure_result {
                Ok((level, current, high)) => {
                    if level > DegradationLevel::Normal {
                        let mut mgr = manager.lock().await;
                        let current_level = mgr
                            .get_tool_call(pid)
                            .map(|tc| tc.degradation)
                            .unwrap_or(DegradationLevel::Normal);

                        // Only escalate, don't de-escalate automatically
                        // (de-escalation happens on tool exit or explicit unfreeze)
                        if level > current_level {
                            if let Err(e) = mgr.apply_degradation(pid, level) {
                                warn!("Failed to apply degradation to pid={}: {}", pid, e);
                            } else {
                                info!(
                                    "⚠️  Degradation: pid={} level={} (mem={}MB/{}MB)",
                                    pid,
                                    level,
                                    current / (1024 * 1024),
                                    high / (1024 * 1024)
                                );
                                metrics.record_throttle(level, 0);
                                metrics.set_degradation(pid, 0, level);

                                if level >= DegradationLevel::Freeze {
                                    metrics.record_freeze();
                                }
                                if level >= DegradationLevel::Terminate {
                                    metrics.record_oom();
                                }
                            }
                        }
                    }
                }
                Err(_) => {
                    // Tool call may have exited between snapshot and check
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_tool_type(s: &str) -> ToolType {
    match s {
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
    }
}

fn parse_priority(s: &str) -> Priority {
    match s {
        "low" => Priority::Low,
        "high" => Priority::High,
        _ => Priority::Medium,
    }
}
