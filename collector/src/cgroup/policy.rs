// AgentCgroup: Policy engine
//
// Parses YAML configuration and provides runtime policy decisions.
// Policies define:
//   - Per-workload resource budgets
//   - Degradation thresholds
//   - Tool-type-specific overrides
//   - Priority assignments
//
// YAML config schema matches Appendix A of the PRD.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use log::{info, warn};

use super::types::*;

// ---------------------------------------------------------------------------
// Top-level configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCgroupConfig {
    #[serde(default = "default_version")]
    pub version: String,

    #[serde(default)]
    pub workloads: Vec<WorkloadPolicy>,

    #[serde(default)]
    pub global: GlobalConfig,

    /// Tool-type-specific memory overrides
    #[serde(default)]
    pub tool_profiles: HashMap<String, ToolProfile>,
}

fn default_version() -> String {
    "0.1".to_string()
}

impl Default for AgentCgroupConfig {
    fn default() -> Self {
        AgentCgroupConfig {
            version: default_version(),
            workloads: Vec::new(),
            global: GlobalConfig::default(),
            tool_profiles: HashMap::new(),
        }
    }
}

impl AgentCgroupConfig {
    /// Load configuration from a YAML file.
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            ConfigError::Io(format!("Failed to read config file {}: {}", path.display(), e))
        })?;
        Self::from_str(&content)
    }

    /// Parse configuration from a YAML string.
    pub fn from_str(yaml: &str) -> Result<Self, ConfigError> {
        serde_yaml::from_str(yaml).map_err(|e| {
            ConfigError::Parse(format!("Failed to parse YAML config: {}", e))
        })
    }

    /// Create a default configuration with a single workload.
    pub fn default_for_pid(pid: u32, agent_binary: &str) -> Self {
        let workload = WorkloadPolicy {
            id: "default".to_string(),
            agent_binary: agent_binary.to_string(),
            priority: Priority::Medium,
            baseline_memory_mb: DEFAULT_BASELINE_MEMORY_MB,
            tool_high_mb: DEFAULT_TOOL_HIGH_MB,
            tool_max_mb: DEFAULT_TOOL_MAX_MB,
            total_memory_max_mb: 4096,
            degradation: DegradationConfig::default(),
            enabled_tools: vec!["bash".into(), "python".into(), "npm".into(), "cargo".into()],
            target_pid: Some(pid),
        };

        AgentCgroupConfig {
            workloads: vec![workload],
            ..Default::default()
        }
    }

    /// Look up the policy for a specific tool type.
    pub fn tool_limits(&self, tool: ToolType) -> ToolMemoryLimits {
        let key = tool.to_string();
        if let Some(profile) = self.tool_profiles.get(&key) {
            ToolMemoryLimits {
                memory_high_mb: profile.memory_high_mb,
                memory_max_mb: profile.memory_max_mb,
            }
        } else {
            ToolMemoryLimits::for_tool(tool)
        }
    }
}

// ---------------------------------------------------------------------------
// Per-workload policy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadPolicy {
    /// Unique workload identifier (for config reference)
    pub id: String,

    /// Path to agent binary (used for process matching)
    #[serde(default)]
    pub agent_binary: String,

    /// Default priority for tool calls from this workload
    #[serde(default = "default_priority")]
    pub priority: Priority,

    /// Framework baseline memory (MB) — stable allocation for Node.js/V8
    #[serde(default = "default_baseline_memory")]
    pub baseline_memory_mb: u64,

    /// Default soft memory limit per tool call (MB)
    #[serde(default = "default_tool_high")]
    pub tool_high_mb: u64,

    /// Default hard memory limit per tool call (MB)
    #[serde(default = "default_tool_max")]
    pub tool_max_mb: u64,

    /// Total memory budget for the entire workload (MB)
    #[serde(default = "default_total_max")]
    pub total_memory_max_mb: u64,

    /// Degradation thresholds and actions
    #[serde(default)]
    pub degradation: DegradationConfig,

    /// Allowed tool commands (empty = allow all)
    #[serde(default)]
    pub enabled_tools: Vec<String>,

    /// Target PID to track (optional, set at runtime)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_pid: Option<u32>,
}

fn default_priority() -> Priority {
    Priority::Medium
}
fn default_baseline_memory() -> u64 {
    DEFAULT_BASELINE_MEMORY_MB
}
fn default_tool_high() -> u64 {
    DEFAULT_TOOL_HIGH_MB
}
fn default_tool_max() -> u64 {
    DEFAULT_TOOL_MAX_MB
}
fn default_total_max() -> u64 {
    4096
}

// ---------------------------------------------------------------------------
// Degradation configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DegradationConfig {
    /// Percentage of memory.high to trigger Level 1 (soft throttle)
    #[serde(default = "default_l1_threshold")]
    pub level_1_threshold: f64,

    /// Delay in ms for Level 1
    #[serde(default = "default_l1_delay")]
    pub level_1_delay_ms: u32,

    /// Percentage of memory.high to trigger Level 2 (hard throttle / freeze)
    #[serde(default = "default_l2_threshold")]
    pub level_2_threshold: f64,

    /// Action for Level 2
    #[serde(default = "default_l2_action")]
    pub level_2_action: String,

    /// Percentage of memory.high to trigger Level 3 (terminate)
    #[serde(default = "default_l3_threshold")]
    pub level_3_threshold: f64,

    /// Action for Level 3
    #[serde(default = "default_l3_action")]
    pub level_3_action: String,
}

fn default_l1_threshold() -> f64 { 0.7 }
fn default_l1_delay() -> u32 { 50 }
fn default_l2_threshold() -> f64 { 0.9 }
fn default_l2_action() -> String { "freeze".to_string() }
fn default_l3_threshold() -> f64 { 0.95 }
fn default_l3_action() -> String { "kill".to_string() }

impl Default for DegradationConfig {
    fn default() -> Self {
        DegradationConfig {
            level_1_threshold: default_l1_threshold(),
            level_1_delay_ms: default_l1_delay(),
            level_2_threshold: default_l2_threshold(),
            level_2_action: default_l2_action(),
            level_3_threshold: default_l3_threshold(),
            level_3_action: default_l3_action(),
        }
    }
}

impl DegradationConfig {
    /// Determine the degradation level from a pressure ratio (current/high).
    pub fn evaluate(&self, pressure_ratio: f64) -> DegradationLevel {
        if pressure_ratio >= self.level_3_threshold {
            match self.level_3_action.as_str() {
                "kill" | "terminate" => DegradationLevel::Terminate,
                "freeze" => DegradationLevel::Freeze,
                _ => DegradationLevel::Terminate,
            }
        } else if pressure_ratio >= self.level_2_threshold {
            match self.level_2_action.as_str() {
                "freeze" => DegradationLevel::Freeze,
                "throttle" => DegradationLevel::HardThrottle,
                _ => DegradationLevel::Freeze,
            }
        } else if pressure_ratio >= self.level_1_threshold {
            DegradationLevel::SoftThrottle
        } else {
            DegradationLevel::Normal
        }
    }
}

// ---------------------------------------------------------------------------
// Tool profile overrides
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolProfile {
    pub memory_high_mb: u64,
    pub memory_max_mb: u64,
    #[serde(default)]
    pub priority: Option<Priority>,
    #[serde(default)]
    pub cpu_weight: Option<u32>,
}

// ---------------------------------------------------------------------------
// Global daemon configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalConfig {
    /// Prometheus metrics HTTP port
    #[serde(default = "default_metrics_port")]
    pub metrics_port: u16,

    /// Prometheus metrics path
    #[serde(default = "default_metrics_path")]
    pub metrics_path: String,

    /// Log level (TRACE, DEBUG, INFO, WARN, ERROR)
    #[serde(default = "default_log_level")]
    pub log_level: String,

    /// Log file path
    #[serde(default = "default_log_file")]
    pub log_file: String,

    /// Pressure monitoring interval (ms)
    #[serde(default = "default_monitor_interval")]
    pub monitor_interval_ms: u64,

    /// Enable automatic degradation enforcement
    #[serde(default = "default_auto_degrade")]
    pub auto_degrade: bool,
}

fn default_metrics_port() -> u16 { 9090 }
fn default_metrics_path() -> String { "/metrics".to_string() }
fn default_log_level() -> String { "INFO".to_string() }
fn default_log_file() -> String { "/var/log/agentcgroup/daemon.log".to_string() }
fn default_monitor_interval() -> u64 { 500 }
fn default_auto_degrade() -> bool { true }

impl Default for GlobalConfig {
    fn default() -> Self {
        GlobalConfig {
            metrics_port: default_metrics_port(),
            metrics_path: default_metrics_path(),
            log_level: default_log_level(),
            log_file: default_log_file(),
            monitor_interval_ms: default_monitor_interval(),
            auto_degrade: default_auto_degrade(),
        }
    }
}

// ---------------------------------------------------------------------------
// PolicyEngine: runtime policy evaluator
// ---------------------------------------------------------------------------

pub struct PolicyEngine {
    config: AgentCgroupConfig,
}

impl PolicyEngine {
    pub fn new(config: AgentCgroupConfig) -> Self {
        info!(
            "PolicyEngine initialized with {} workloads, {} tool profiles",
            config.workloads.len(),
            config.tool_profiles.len()
        );
        PolicyEngine { config }
    }

    pub fn config(&self) -> &AgentCgroupConfig {
        &self.config
    }

    /// Find the workload policy for a given agent binary name.
    pub fn find_workload_policy(&self, agent_binary: &str) -> Option<&WorkloadPolicy> {
        self.config.workloads.iter().find(|w| {
            w.agent_binary == agent_binary
                || agent_binary.contains(&w.agent_binary)
                || w.agent_binary.contains(agent_binary)
        })
    }

    /// Find the workload policy by ID.
    pub fn find_workload_by_id(&self, id: &str) -> Option<&WorkloadPolicy> {
        self.config.workloads.iter().find(|w| w.id == id)
    }

    /// Get the first workload policy (for single-workload mode).
    pub fn default_workload(&self) -> Option<&WorkloadPolicy> {
        self.config.workloads.first()
    }

    /// Determine the memory limits for a tool call.
    pub fn tool_limits(&self, tool_type: ToolType) -> ToolMemoryLimits {
        self.config.tool_limits(tool_type)
    }

    /// Evaluate degradation level for a workload's tool call.
    pub fn evaluate_degradation(
        &self,
        workload_id: &str,
        memory_current: u64,
        memory_high: u64,
    ) -> DegradationLevel {
        if memory_high == 0 {
            return DegradationLevel::Normal;
        }

        let ratio = memory_current as f64 / memory_high as f64;

        if let Some(wl) = self.find_workload_by_id(workload_id) {
            wl.degradation.evaluate(ratio)
        } else {
            // Use default thresholds
            DegradationConfig::default().evaluate(ratio)
        }
    }

    /// Update the config at runtime (for dynamic policy updates).
    pub fn update_config(&mut self, config: AgentCgroupConfig) {
        info!("PolicyEngine config updated");
        self.config = config;
    }
}

// ---------------------------------------------------------------------------
// Config errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ConfigError {
    Io(String),
    Parse(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(s) => write!(f, "Config I/O error: {}", s),
            ConfigError::Parse(s) => write!(f, "Config parse error: {}", s),
        }
    }
}

impl std::error::Error for ConfigError {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_config() {
        let yaml = r#"
version: "0.1"
workloads:
  - id: "test-agent"
    agent_binary: "/usr/bin/claude-code"
    priority: high
    baseline_memory_mb: 185
    tool_high_mb: 500
    tool_max_mb: 2048
"#;
        let config = AgentCgroupConfig::from_str(yaml).unwrap();
        assert_eq!(config.workloads.len(), 1);
        assert_eq!(config.workloads[0].id, "test-agent");
        assert_eq!(config.workloads[0].priority, Priority::High);
        assert_eq!(config.workloads[0].baseline_memory_mb, 185);
    }

    #[test]
    fn test_degradation_evaluation() {
        let config = DegradationConfig::default();

        assert_eq!(config.evaluate(0.5), DegradationLevel::Normal);
        assert_eq!(config.evaluate(0.75), DegradationLevel::SoftThrottle);
        assert_eq!(config.evaluate(0.92), DegradationLevel::Freeze);
        assert_eq!(config.evaluate(0.96), DegradationLevel::Terminate);
    }

    #[test]
    fn test_default_config_for_pid() {
        let config = AgentCgroupConfig::default_for_pid(1234, "claude");
        assert_eq!(config.workloads.len(), 1);
        assert_eq!(config.workloads[0].target_pid, Some(1234));
    }

    #[test]
    fn test_tool_profile_override() {
        let yaml = r#"
version: "0.1"
workloads: []
tool_profiles:
  test:
    memory_high_mb: 1024
    memory_max_mb: 4096
"#;
        let config = AgentCgroupConfig::from_str(yaml).unwrap();
        let limits = config.tool_limits(ToolType::Test);
        assert_eq!(limits.memory_high_mb, 1024);
        assert_eq!(limits.memory_max_mb, 4096);
    }
}
