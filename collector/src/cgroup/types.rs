// AgentCgroup: Core types for the resource controller
//
// Defines shared types used across the cgroup management subsystem:
// - Priority tiers, tool types, degradation levels
// - Configuration structures
// - Event types from BPF programs
// - Prometheus metric identifiers

use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// Priority tiers (mirroring BPF enum tool_priority)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Low = 0,
    Medium = 1,
    High = 2,
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Priority::Low => write!(f, "low"),
            Priority::Medium => write!(f, "medium"),
            Priority::High => write!(f, "high"),
        }
    }
}

impl From<u32> for Priority {
    fn from(v: u32) -> Self {
        match v {
            0 => Priority::Low,
            2 => Priority::High,
            _ => Priority::Medium,
        }
    }
}

// ---------------------------------------------------------------------------
// Tool types (mirroring BPF enum tool_type)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolType {
    Unknown = 0,
    Bash = 1,
    Test = 2,
    Build = 3,
    Install = 4,
    Git = 5,
    Editor = 6,
    Python = 7,
    Node = 8,
    Lint = 9,
    Format = 10,
}

impl fmt::Display for ToolType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolType::Unknown => write!(f, "unknown"),
            ToolType::Bash => write!(f, "bash"),
            ToolType::Test => write!(f, "test"),
            ToolType::Build => write!(f, "build"),
            ToolType::Install => write!(f, "install"),
            ToolType::Git => write!(f, "git"),
            ToolType::Editor => write!(f, "editor"),
            ToolType::Python => write!(f, "python"),
            ToolType::Node => write!(f, "node"),
            ToolType::Lint => write!(f, "lint"),
            ToolType::Format => write!(f, "format"),
        }
    }
}

impl From<u32> for ToolType {
    fn from(v: u32) -> Self {
        match v {
            1 => ToolType::Bash,
            2 => ToolType::Test,
            3 => ToolType::Build,
            4 => ToolType::Install,
            5 => ToolType::Git,
            6 => ToolType::Editor,
            7 => ToolType::Python,
            8 => ToolType::Node,
            9 => ToolType::Lint,
            10 => ToolType::Format,
            _ => ToolType::Unknown,
        }
    }
}

// ---------------------------------------------------------------------------
// Degradation levels (mirroring BPF enum degradation_level)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DegradationLevel {
    Normal = 0,
    SoftThrottle = 1,
    HardThrottle = 2,
    Freeze = 3,
    Terminate = 4,
}

impl fmt::Display for DegradationLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DegradationLevel::Normal => write!(f, "normal"),
            DegradationLevel::SoftThrottle => write!(f, "soft_throttle"),
            DegradationLevel::HardThrottle => write!(f, "hard_throttle"),
            DegradationLevel::Freeze => write!(f, "freeze"),
            DegradationLevel::Terminate => write!(f, "terminate"),
        }
    }
}

impl From<u32> for DegradationLevel {
    fn from(v: u32) -> Self {
        match v {
            0 => DegradationLevel::Normal,
            1 => DegradationLevel::SoftThrottle,
            2 => DegradationLevel::HardThrottle,
            3 => DegradationLevel::Freeze,
            4 => DegradationLevel::Terminate,
            _ => DegradationLevel::Normal,
        }
    }
}

// ---------------------------------------------------------------------------
// Tool-call event from BPF ring buffer (parsed from JSON)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub timestamp_ns: u64,
    pub pid: u32,
    pub ppid: u32,
    pub workload_id: u32,
    pub tool_type: String,
    pub priority: String,
    pub exit_code: u32,
    pub duration_ns: u64,
    pub memory_usage_bytes: u64,
    pub memory_limit_bytes: u64,
    pub degradation_level: String,
    pub comm: String,
    pub full_command: String,
}

impl ToolCallEvent {
    /// Returns true if this is a tool-call start event
    pub fn is_start(&self) -> bool {
        self.event_type == "tool_start"
    }

    /// Returns true if this is a tool-call exit event
    pub fn is_exit(&self) -> bool {
        self.event_type == "tool_exit"
    }

    /// Parse the tool type string into the enum
    pub fn parsed_tool_type(&self) -> ToolType {
        match self.tool_type.as_str() {
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

    /// Parse the priority string into the enum
    pub fn parsed_priority(&self) -> Priority {
        match self.priority.as_str() {
            "low" => Priority::Low,
            "high" => Priority::High,
            _ => Priority::Medium,
        }
    }
}

// ---------------------------------------------------------------------------
// Workload descriptor: represents one AI agent instance
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadInfo {
    pub id: u32,
    pub root_pid: u32,
    pub agent_binary: String,
    pub cgroup_path: String,
    pub priority: Priority,
    pub active_tool_calls: u32,
    pub total_tool_calls: u64,
    pub current_degradation: DegradationLevel,
}

// ---------------------------------------------------------------------------
// Tool-call descriptor: one active subprocess
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallInfo {
    pub pid: u32,
    pub ppid: u32,
    pub workload_id: u32,
    pub tool_type: ToolType,
    pub priority: Priority,
    pub cgroup_path: String,
    pub start_time_ns: u64,
    pub memory_current_bytes: u64,
    pub memory_high_bytes: u64,
    pub memory_max_bytes: u64,
    pub degradation: DegradationLevel,
}

// ---------------------------------------------------------------------------
// Memory limit presets per tool type (from paper measurements)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolMemoryLimits {
    pub memory_high_mb: u64,
    pub memory_max_mb: u64,
}

impl ToolMemoryLimits {
    /// Default limits based on paper measurements
    pub fn for_tool(tool: ToolType) -> Self {
        match tool {
            ToolType::Test => ToolMemoryLimits {
                memory_high_mb: 518,   // pytest P95
                memory_max_mb: 2048,
            },
            ToolType::Build => ToolMemoryLimits {
                memory_high_mb: 400,
                memory_max_mb: 2048,
            },
            ToolType::Install => ToolMemoryLimits {
                memory_high_mb: 233,
                memory_max_mb: 500,
            },
            ToolType::Git => ToolMemoryLimits {
                memory_high_mb: 14,
                memory_max_mb: 50,
            },
            ToolType::Python => ToolMemoryLimits {
                memory_high_mb: 300,
                memory_max_mb: 1024,
            },
            ToolType::Node => ToolMemoryLimits {
                memory_high_mb: 300,
                memory_max_mb: 1024,
            },
            ToolType::Bash => ToolMemoryLimits {
                memory_high_mb: 200,
                memory_max_mb: 1024,
            },
            _ => ToolMemoryLimits {
                memory_high_mb: 500,   // Conservative default
                memory_max_mb: 2048,
            },
        }
    }

    pub fn memory_high_bytes(&self) -> u64 {
        self.memory_high_mb * 1024 * 1024
    }

    pub fn memory_max_bytes(&self) -> u64 {
        self.memory_max_mb * 1024 * 1024
    }
}

// ---------------------------------------------------------------------------
// Cgroup operation results
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum CgroupError {
    Io(std::io::Error),
    NotFound(String),
    PermissionDenied(String),
    InvalidState(String),
    AlreadyExists(String),
}

impl fmt::Display for CgroupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CgroupError::Io(e) => write!(f, "I/O error: {}", e),
            CgroupError::NotFound(s) => write!(f, "Not found: {}", s),
            CgroupError::PermissionDenied(s) => write!(f, "Permission denied: {}", s),
            CgroupError::InvalidState(s) => write!(f, "Invalid state: {}", s),
            CgroupError::AlreadyExists(s) => write!(f, "Already exists: {}", s),
        }
    }
}

impl std::error::Error for CgroupError {}

impl From<std::io::Error> for CgroupError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound => CgroupError::NotFound(e.to_string()),
            std::io::ErrorKind::PermissionDenied => CgroupError::PermissionDenied(e.to_string()),
            std::io::ErrorKind::AlreadyExists => CgroupError::AlreadyExists(e.to_string()),
            _ => CgroupError::Io(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Constants (matching BPF header defaults)
// ---------------------------------------------------------------------------

pub const DEFAULT_BASELINE_MEMORY_MB: u64 = 185;
pub const DEFAULT_TOOL_HIGH_MB: u64 = 500;
pub const DEFAULT_TOOL_MAX_MB: u64 = 2048;

pub const DEFAULT_LEVEL_1_THRESHOLD: u32 = 70;
pub const DEFAULT_LEVEL_2_THRESHOLD: u32 = 90;
pub const DEFAULT_LEVEL_3_THRESHOLD: u32 = 95;

pub const CGROUP_ROOT: &str = "/sys/fs/cgroup";
pub const AGENTCGROUP_ROOT: &str = "agentcgroup";
