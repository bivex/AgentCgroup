// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// AgentCgroup: Tool-call-aligned resource controller for AI coding agents
//
// eBPF kernel-space program for:
//   1. Tool-call boundary detection (fork/exec/exit tracepoints)
//   2. Tool-type classification from command-line heuristics
//   3. Per-tool-call metadata tracking in BPF maps
//   4. Event emission to userspace daemon via ring buffer
//
// This program detects when an AI agent framework spawns subprocesses
// for tool calls, classifies the tool type, and emits structured events
// that the userspace daemon uses to create/manage cgroup hierarchies.

#include "vmlinux.h"
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include <bpf/bpf_core_read.h>
#include "agentcgroup.h"

// --------------------------------------------------------------------------
// Configuration (set from userspace via rodata)
// --------------------------------------------------------------------------
const volatile __u32 target_workload_id = 0;   // 0 = track all workloads
const volatile __u32 target_pid = 0;           // 0 = track all PIDs
const volatile __u64 min_duration_ns = 0;      // Minimum duration to report

// --------------------------------------------------------------------------
// BPF Maps
// --------------------------------------------------------------------------

// Ring buffer for sending events to userspace (256 KB)
struct {
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, 256 * 1024);
} events SEC(".maps");

// Per-PID tool-call metadata (populated on fork/exec, read on exit)
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, MAX_TOOL_CALLS);
    __type(key, __u32);
    __type(value, struct tool_call_meta);
} tool_call_map SEC(".maps");

// Workload registry: maps root PID → workload metadata
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, MAX_WORKLOADS);
    __type(key, __u32);
    __type(value, struct workload_meta);
} workload_map SEC(".maps");

// Policy parameters: indexed by priority tier (0=LOW, 1=MED, 2=HIGH)
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, MAX_POLICY_ENTRIES);
    __type(key, __u32);
    __type(value, struct policy_params);
} policy_map SEC(".maps");

// Per-CPU metrics counters
struct {
    __uint(type, BPF_MAP_TYPE_PERCPU_ARRAY);
    __uint(max_entries, __METRIC_MAX);
    __type(key, __u32);
    __type(value, __u64);
} metrics_map SEC(".maps");

// Exec start times: maps PID → timestamp for duration calculation
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, MAX_TOOL_CALLS);
    __type(key, __u32);
    __type(value, __u64);
} exec_start SEC(".maps");

// Tracked parent PIDs: set of PIDs belonging to tracked workloads
// Value: workload_id. Used to determine if a forked child should be tracked.
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, MAX_TOOL_CALLS * 4);
    __type(key, __u32);
    __type(value, __u32);
} tracked_pids SEC(".maps");

// --------------------------------------------------------------------------
// Helper: Increment a per-CPU metric counter
// --------------------------------------------------------------------------
static __always_inline void metric_inc(enum metrics_key key)
{
    __u32 k = key;
    __u64 *val = bpf_map_lookup_elem(&metrics_map, &k);
    if (val)
        __sync_fetch_and_add(val, 1);
}

// --------------------------------------------------------------------------
// Helper: Classify tool type from comm string
// Uses simple prefix/substring matching on the process comm.
// --------------------------------------------------------------------------
static __always_inline enum tool_type classify_tool(const char *comm)
{
    // Check for test runners
    if (comm[0] == 'p' && comm[1] == 'y' && comm[2] == 't' &&
        comm[3] == 'e' && comm[4] == 's' && comm[5] == 't')
        return TOOL_TEST;

    // cargo test → comm is "cargo"
    if (comm[0] == 'c' && comm[1] == 'a' && comm[2] == 'r' &&
        comm[3] == 'g' && comm[4] == 'o')
        return TOOL_BUILD;  // Could be build or test, refine in userspace

    // npm / npx
    if (comm[0] == 'n' && comm[1] == 'p' && comm[2] == 'm')
        return TOOL_BUILD;
    if (comm[0] == 'n' && comm[1] == 'p' && comm[2] == 'x')
        return TOOL_BUILD;

    // pip
    if (comm[0] == 'p' && comm[1] == 'i' && comm[2] == 'p')
        return TOOL_INSTALL;

    // git
    if (comm[0] == 'g' && comm[1] == 'i' && comm[2] == 't')
        return TOOL_GIT;

    // python / python3
    if (comm[0] == 'p' && comm[1] == 'y' && comm[2] == 't' &&
        comm[3] == 'h' && comm[4] == 'o' && comm[5] == 'n')
        return TOOL_PYTHON;

    // node
    if (comm[0] == 'n' && comm[1] == 'o' && comm[2] == 'd' &&
        comm[3] == 'e')
        return TOOL_NODE;

    // make
    if (comm[0] == 'm' && comm[1] == 'a' && comm[2] == 'k' &&
        comm[3] == 'e')
        return TOOL_BUILD;

    // gcc / g++
    if (comm[0] == 'g' && comm[1] == 'c' && comm[2] == 'c')
        return TOOL_BUILD;
    if (comm[0] == 'g' && comm[1] == '+' && comm[2] == '+')
        return TOOL_BUILD;

    // clang
    if (comm[0] == 'c' && comm[1] == 'l' && comm[2] == 'a' &&
        comm[3] == 'n' && comm[4] == 'g')
        return TOOL_BUILD;

    // sed, awk, patch
    if (comm[0] == 's' && comm[1] == 'e' && comm[2] == 'd')
        return TOOL_EDITOR;
    if (comm[0] == 'a' && comm[1] == 'w' && comm[2] == 'k')
        return TOOL_EDITOR;
    if (comm[0] == 'p' && comm[1] == 'a' && comm[2] == 't' &&
        comm[3] == 'c' && comm[4] == 'h')
        return TOOL_EDITOR;

    // bash / sh / zsh
    if (comm[0] == 'b' && comm[1] == 'a' && comm[2] == 's' &&
        comm[3] == 'h')
        return TOOL_BASH;
    if (comm[0] == 's' && comm[1] == 'h' && comm[2] == '\0')
        return TOOL_BASH;
    if (comm[0] == 'z' && comm[1] == 's' && comm[2] == 'h')
        return TOOL_BASH;

    return TOOL_UNKNOWN;
}

// --------------------------------------------------------------------------
// Helper: Determine priority based on tool type
// Test execution and compilation get HIGH priority; git/editor get LOW.
// --------------------------------------------------------------------------
static __always_inline enum tool_priority priority_for_tool(enum tool_type type)
{
    switch (type) {
    case TOOL_TEST:
    case TOOL_BUILD:
        return PRIORITY_HIGH;
    case TOOL_INSTALL:
    case TOOL_PYTHON:
    case TOOL_NODE:
    case TOOL_BASH:
        return PRIORITY_MEDIUM;
    case TOOL_GIT:
    case TOOL_EDITOR:
    case TOOL_LINT:
    case TOOL_FORMAT:
        return PRIORITY_LOW;
    default:
        return PRIORITY_MEDIUM;
    }
}

// --------------------------------------------------------------------------
// Helper: Check if a PID belongs to a tracked workload
// Returns the workload_id or 0 if not tracked.
// --------------------------------------------------------------------------
static __always_inline __u32 get_workload_for_pid(__u32 pid)
{
    __u32 *wid = bpf_map_lookup_elem(&tracked_pids, &pid);
    return wid ? *wid : 0;
}

// --------------------------------------------------------------------------
// Helper: Emit an event to the ring buffer
// --------------------------------------------------------------------------
static __always_inline int emit_event(
    enum agentcgroup_event_type type,
    __u32 pid, __u32 ppid, __u32 workload_id,
    enum tool_type tool, enum tool_priority priority,
    const char *comm)
{
    struct agentcgroup_event *evt;

    evt = bpf_ringbuf_reserve(&events, sizeof(*evt), 0);
    if (!evt)
        return -1;

    evt->type = type;
    evt->timestamp_ns = bpf_ktime_get_ns();
    evt->pid = pid;
    evt->ppid = ppid;
    evt->workload_id = workload_id;
    evt->tool = tool;
    evt->priority = priority;
    evt->exit_code = 0;
    evt->duration_ns = 0;
    evt->memory_usage_bytes = 0;
    evt->memory_limit_bytes = 0;
    evt->degrade_level = DEGRADE_NORMAL;

    // Copy comm safely
    bpf_probe_read_kernel_str(evt->comm, sizeof(evt->comm), comm);

    bpf_ringbuf_submit(evt, 0);
    return 0;
}

// ==========================================================================
// Tracepoint: sched_process_exec
//   Fired when a process calls exec(). This is the primary detection point
//   for tool calls — the agent framework forks a child, then the child
//   exec()s the tool (bash, pytest, pip, git, etc.).
// ==========================================================================
SEC("tp/sched/sched_process_exec")
int handle_tool_exec(struct trace_event_raw_sched_process_exec *ctx)
{
    __u32 pid = bpf_get_current_pid_tgid() >> 32;
    __u32 tid = (__u32)bpf_get_current_pid_tgid();

    // Only track main thread
    if (pid != tid)
        return 0;

    // Filter by target PID if set
    if (target_pid && pid != target_pid)
        return 0;

    struct task_struct *task = (struct task_struct *)bpf_get_current_task();
    __u32 ppid = BPF_CORE_READ(task, real_parent, tgid);

    // Check if parent is in a tracked workload
    __u32 workload_id = get_workload_for_pid(ppid);

    // Also check if this PID itself is already tracked (re-exec)
    if (!workload_id)
        workload_id = get_workload_for_pid(pid);

    // If we have a target workload filter, apply it
    if (target_workload_id && workload_id != target_workload_id)
        return 0;

    // If not in any tracked workload and no workload filter, still track
    // (the daemon will decide whether to adopt this process)
    if (!workload_id && target_workload_id)
        return 0;

    // Read comm for tool classification
    char comm[TASK_COMM_LEN];
    bpf_get_current_comm(comm, sizeof(comm));

    enum tool_type tool = classify_tool(comm);
    enum tool_priority priority = priority_for_tool(tool);

    // Record exec start time
    __u64 ts = bpf_ktime_get_ns();
    bpf_map_update_elem(&exec_start, &pid, &ts, BPF_ANY);

    // Build tool-call metadata
    struct tool_call_meta meta = {};
    meta.pid = pid;
    meta.ppid = ppid;
    meta.workload_id = workload_id;
    meta.type = tool;
    meta.priority = priority;
    meta.start_time_ns = ts;
    meta.cpu_time_ns = 0;
    meta.peak_memory_bytes = 0;
    bpf_get_current_comm(meta.comm, sizeof(meta.comm));

    // Read full command line from mm->arg_start
    struct mm_struct *mm = BPF_CORE_READ(task, mm);
    if (mm) {
        unsigned long arg_start = BPF_CORE_READ(mm, arg_start);
        unsigned long arg_end = BPF_CORE_READ(mm, arg_end);
        unsigned long arg_len = arg_end - arg_start;
        if (arg_len > MAX_COMMAND_LEN - 1)
            arg_len = MAX_COMMAND_LEN - 1;
        bpf_probe_read_user(meta.full_command, arg_len & (MAX_COMMAND_LEN - 1),
                            (void *)arg_start);
    }

    // Store metadata
    bpf_map_update_elem(&tool_call_map, &pid, &meta, BPF_ANY);

    // Track this PID as belonging to the workload
    if (workload_id)
        bpf_map_update_elem(&tracked_pids, &pid, &workload_id, BPF_ANY);

    // Emit event
    emit_event(ACGROUP_EVENT_TOOL_START, pid, ppid, workload_id,
               tool, priority, comm);

    metric_inc(METRIC_TOOL_CALLS_STARTED);

    return 0;
}

// ==========================================================================
// Tracepoint: sched_process_fork
//   Track child processes of tracked workloads so we can assign them
//   to the same cgroup hierarchy.
// ==========================================================================
SEC("tp/sched/sched_process_fork")
int handle_fork(struct trace_event_raw_sched_process_fork *ctx)
{
    __u32 parent_pid = ctx->parent_pid;
    __u32 child_pid = ctx->child_pid;

    // Check if parent is tracked
    __u32 workload_id = get_workload_for_pid(parent_pid);
    if (!workload_id)
        return 0;

    // Inherit workload tracking to child
    bpf_map_update_elem(&tracked_pids, &child_pid, &workload_id, BPF_ANY);

    return 0;
}

// ==========================================================================
// Tracepoint: sched_process_exit
//   Fired when a process exits. Records metrics, emits exit event,
//   and cleans up BPF map entries.
// ==========================================================================
SEC("tp/sched/sched_process_exit")
int handle_tool_exit(struct trace_event_raw_sched_process_template *ctx)
{
    __u32 pid = bpf_get_current_pid_tgid() >> 32;
    __u32 tid = (__u32)bpf_get_current_pid_tgid();

    // Only track main thread
    if (pid != tid)
        return 0;

    // Look up tool-call metadata
    struct tool_call_meta *meta = bpf_map_lookup_elem(&tool_call_map, &pid);
    if (!meta) {
        // Not a tracked tool call, clean up tracking if present
        bpf_map_delete_elem(&tracked_pids, &pid);
        return 0;
    }

    __u64 ts = bpf_ktime_get_ns();
    __u64 duration = ts - meta->start_time_ns;

    // Apply minimum duration filter
    if (min_duration_ns && duration < min_duration_ns)
        goto cleanup;

    // Read exit code
    struct task_struct *task = (struct task_struct *)bpf_get_current_task();
    __u32 exit_code = BPF_CORE_READ(task, exit_code) >> 8;

    // Emit exit event with duration and metrics
    struct agentcgroup_event *evt;
    evt = bpf_ringbuf_reserve(&events, sizeof(*evt), 0);
    if (evt) {
        evt->type = ACGROUP_EVENT_TOOL_EXIT;
        evt->timestamp_ns = ts;
        evt->pid = pid;
        evt->ppid = meta->ppid;
        evt->workload_id = meta->workload_id;
        evt->tool = meta->type;
        evt->priority = meta->priority;
        evt->exit_code = exit_code;
        evt->duration_ns = duration;
        evt->memory_usage_bytes = meta->peak_memory_bytes;
        evt->memory_limit_bytes = 0;
        evt->degrade_level = DEGRADE_NORMAL;
        bpf_probe_read_kernel_str(evt->comm, sizeof(evt->comm), meta->comm);
        bpf_probe_read_kernel(evt->full_command, sizeof(evt->full_command),
                              meta->full_command);
        bpf_ringbuf_submit(evt, 0);
    }

    metric_inc(METRIC_TOOL_CALLS_EXITED);

cleanup:
    // Clean up all maps
    bpf_map_delete_elem(&tool_call_map, &pid);
    bpf_map_delete_elem(&exec_start, &pid);
    bpf_map_delete_elem(&tracked_pids, &pid);

    return 0;
}

char LICENSE[] SEC("license") = "Dual BSD/GPL";
