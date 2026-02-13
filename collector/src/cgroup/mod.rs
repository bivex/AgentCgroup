// AgentCgroup: cgroup v2 resource controller module
//
// This module implements the AgentCgroup resource management system,
// providing tool-call-aligned cgroup hierarchies, graceful degradation
// policies, and Prometheus metrics for AI coding agent workloads.
//
// Architecture:
//
//   ┌─────────────────────────────────────────────────────┐
//   │  CLI (main.rs)                                      │
//   │  `agentcgroup daemon|monitor|policy|metrics`        │
//   └──────────────┬──────────────────────────────────────┘
//                   │
//   ┌───────────────▼──────────────────────────────────────┐
//   │  Daemon (daemon.rs)                                  │
//   │  Orchestrates all components                         │
//   │  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ │
//   │  │ PolicyEngine │ │CgroupManager │ │  Metrics     │ │
//   │  │ (policy.rs)  │ │(manager.rs)  │ │(metrics.rs)  │ │
//   │  └──────────────┘ └──────────────┘ └──────────────┘ │
//   │  ┌──────────────────────────────────────────────────┐│
//   │  │ ToolCallTracker (tracker.rs)                     ││
//   │  │ Consumes BPF events, drives cgroup ops           ││
//   │  └──────────────────────────────────────────────────┘│
//   └──────────────────────────────────────────────────────┘
//                   │
//   ┌───────────────▼──────────────────────────────────────┐
//   │  cgroup v2 filesystem                                │
//   │  /sys/fs/cgroup/agentcgroup/                         │
//   │    ├── workload-1/baseline/                          │
//   │    ├── workload-1/tool-{pid}/                        │
//   │    └── ...                                           │
//   └──────────────────────────────────────────────────────┘

pub mod types;
pub mod manager;
pub mod policy;
pub mod metrics;
pub mod tracker;
pub mod daemon;

// Re-export key types for convenience
pub use types::{
    Priority, ToolType, DegradationLevel,
    ToolCallEvent, WorkloadInfo, ToolCallInfo,
    ToolMemoryLimits, CgroupError,
};
pub use manager::CgroupManager;
pub use policy::{AgentCgroupConfig, PolicyEngine, WorkloadPolicy, DegradationConfig};
pub use metrics::{MetricsCollector, MetricsServer};
pub use tracker::{ToolCallTracker, PressureMonitorAnalyzer};
pub use daemon::{Daemon, DaemonConfig};
