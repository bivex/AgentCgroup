// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// AgentCgroup: Tool-call-aligned resource controller for AI coding agents
//
// Shared header between eBPF kernel programs and userspace daemon.
// Defines events, maps, and constants for tool-call tracking and
// cgroup-based resource enforcement.

#ifndef __AGENTCGROUP_H
#define __AGENTCGROUP_H

#define TASK_COMM_LEN       16
#define MAX_COMMAND_LEN     256
#define MAX_FILENAME_LEN    127
#define MAX_CGROUP_PATH     256

// Maximum concurrent tracked tool calls per agent
#define MAX_TOOL_CALLS      1024
// Maximum concurrent workloads (agents)
#define MAX_WORKLOADS       128
// Maximum policy entries (one per priority tier)
#define MAX_POLICY_ENTRIES  3

// --------------------------------------------------------------------------
// Priority tiers for tool-call scheduling
// --------------------------------------------------------------------------
enum tool_priority {
    PRIORITY_LOW    = 0,  // Background tasks (git status, ls)
    PRIORITY_MEDIUM = 1,  // Normal tool calls (pip install, cargo build)
    PRIORITY_HIGH   = 2,  // Critical tasks (test execution, compilation)
};

// --------------------------------------------------------------------------
// Tool types detected from command-line heuristics
// --------------------------------------------------------------------------
enum tool_type {
    TOOL_UNKNOWN    = 0,
    TOOL_BASH       = 1,  // Generic bash command
    TOOL_TEST       = 2,  // pytest, cargo test, npm test
    TOOL_BUILD      = 3,  // make, cargo build, npm build
    TOOL_INSTALL    = 4,  // pip install, npm install, cargo install
    TOOL_GIT        = 5,  // git operations
    TOOL_EDITOR     = 6,  // sed, awk, patch
    TOOL_PYTHON     = 7,  // python script execution
    TOOL_NODE       = 8,  // node.js execution
    TOOL_LINT       = 9,  // linting tools
    TOOL_FORMAT     = 10, // formatting tools
};

// --------------------------------------------------------------------------
// Degradation levels for graceful memory pressure response
// --------------------------------------------------------------------------
enum degradation_level {
    DEGRADE_NORMAL       = 0,  // Pressure < 70%, no action
    DEGRADE_SOFT_THROTTLE = 1, // 70-90%, 50ms allocation delay
    DEGRADE_HARD_THROTTLE = 2, // 90-95%, 100ms allocation delay
    DEGRADE_FREEZE       = 3,  // 95-99%, cgroup.freeze
    DEGRADE_TERMINATE    = 4,  // >99%, cgroup.kill (last resort)
};

// --------------------------------------------------------------------------
// Event types sent from kernel to userspace via ring buffer
// --------------------------------------------------------------------------
enum agentcgroup_event_type {
    ACGROUP_EVENT_TOOL_START   = 0,  // Tool-call process started (fork/exec)
    ACGROUP_EVENT_TOOL_EXIT    = 1,  // Tool-call process exited
    ACGROUP_EVENT_MEMORY_HIGH  = 2,  // Memory breached memory.high threshold
    ACGROUP_EVENT_MEMORY_MAX   = 3,  // Memory approaching memory.max
    ACGROUP_EVENT_THROTTLE     = 4,  // Throttling applied
    ACGROUP_EVENT_FREEZE       = 5,  // cgroup frozen
    ACGROUP_EVENT_UNFREEZE     = 6,  // cgroup unfrozen
    ACGROUP_EVENT_OOM          = 7,  // OOM kill occurred
};

// --------------------------------------------------------------------------
// Tool-call metadata stored in BPF hash map (key: PID)
// --------------------------------------------------------------------------
struct tool_call_meta {
    __u32 pid;
    __u32 ppid;
    __u32 workload_id;           // Which agent workload this belongs to
    enum tool_type type;
    enum tool_priority priority;
    __u64 start_time_ns;         // Timestamp of fork/exec
    __u64 cpu_time_ns;           // Accumulated CPU time
    __u64 peak_memory_bytes;     // Peak RSS observed
    char comm[TASK_COMM_LEN];
    char full_command[MAX_COMMAND_LEN];
};

// --------------------------------------------------------------------------
// Policy parameters stored in BPF array map (key: priority tier)
// --------------------------------------------------------------------------
struct policy_params {
    __u64 memory_high_bytes;     // Soft memory limit (triggers throttling)
    __u64 memory_max_bytes;      // Hard memory limit (triggers OOM)
    __u32 level_1_threshold;     // % of memory.high for soft throttle (default: 70)
    __u32 level_2_threshold;     // % of memory.high for hard throttle (default: 90)
    __u32 level_3_threshold;     // % of memory.high for freeze (default: 95)
    __u32 level_1_delay_ms;      // Allocation delay at level 1 (default: 50)
    __u32 level_2_delay_ms;      // Allocation delay at level 2 (default: 100)
    __u32 cpu_weight;            // CPU weight for this priority tier
};

// --------------------------------------------------------------------------
// Workload (agent instance) metadata
// --------------------------------------------------------------------------
struct workload_meta {
    __u32 workload_id;
    __u32 root_pid;              // Agent framework root PID
    __u64 total_memory_bytes;    // Total memory budget
    __u64 baseline_memory_bytes; // Framework baseline allocation (~185MB)
    __u32 active_tool_calls;     // Currently running tool calls
    __u32 total_tool_calls;      // Lifetime count
    char agent_binary[TASK_COMM_LEN];
    char cgroup_path[MAX_CGROUP_PATH];
};

// --------------------------------------------------------------------------
// Event sent from eBPF to userspace via ring buffer
// --------------------------------------------------------------------------
struct agentcgroup_event {
    enum agentcgroup_event_type type;
    __u64 timestamp_ns;
    __u32 pid;
    __u32 ppid;
    __u32 workload_id;
    enum tool_type tool;
    enum tool_priority priority;
    __u32 exit_code;             // Only for TOOL_EXIT events
    __u64 duration_ns;           // Only for TOOL_EXIT events
    __u64 memory_usage_bytes;    // Current memory at event time
    __u64 memory_limit_bytes;    // Applicable limit at event time
    enum degradation_level degrade_level;
    char comm[TASK_COMM_LEN];
    char full_command[MAX_COMMAND_LEN];
};

// --------------------------------------------------------------------------
// Metrics counters (per-CPU array for lock-free updates)
// --------------------------------------------------------------------------
enum metrics_key {
    METRIC_TOOL_CALLS_STARTED  = 0,
    METRIC_TOOL_CALLS_EXITED   = 1,
    METRIC_THROTTLE_EVENTS     = 2,
    METRIC_FREEZE_EVENTS       = 3,
    METRIC_OOM_KILLS           = 4,
    METRIC_MEMORY_HIGH_BREACHES = 5,
    METRIC_TOTAL_CPU_TIME_NS   = 6,
    METRIC_TOTAL_MEMORY_PEAK   = 7,
    __METRIC_MAX               = 8,
};

// --------------------------------------------------------------------------
// Default policy values (from paper measurements)
// --------------------------------------------------------------------------
#define DEFAULT_BASELINE_MEMORY_MB    185   // Framework overhead (Node.js + V8)
#define DEFAULT_TOOL_HIGH_MB          500   // P95 for pytest
#define DEFAULT_TOOL_MAX_MB           2048  // Hard ceiling per tool call
#define DEFAULT_LEVEL_1_THRESHOLD     70    // 70% of memory.high
#define DEFAULT_LEVEL_2_THRESHOLD     90    // 90% of memory.high
#define DEFAULT_LEVEL_3_THRESHOLD     95    // 95% of memory.high
#define DEFAULT_LEVEL_1_DELAY_MS      50
#define DEFAULT_LEVEL_2_DELAY_MS      100

// Tool-type specific memory limits (from paper Table measurements)
#define TOOL_MEM_HIGH_TEST_MB         518   // pytest P95
#define TOOL_MEM_HIGH_BUILD_MB        400   // Compilation P95
#define TOOL_MEM_HIGH_INSTALL_MB      233   // pip install P95
#define TOOL_MEM_HIGH_GIT_MB          14    // git status P95
#define TOOL_MEM_HIGH_BASH_MB         200   // Generic bash P95
#define TOOL_MEM_HIGH_PYTHON_MB       300   // Python script P95

#endif /* __AGENTCGROUP_H */
