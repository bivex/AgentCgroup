// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// AgentCgroup: Userspace loader for tool-call tracking eBPF program
//
// This standalone binary loads the agentcgroup eBPF programs and outputs
// structured JSON events to stdout. It serves two purposes:
//   1. Standalone testing/debugging of tool-call detection
//   2. Embedded binary that the Rust daemon spawns and reads from
//
// Usage:
//   sudo ./agentcgroup                    # Track all processes
//   sudo ./agentcgroup -p <pid>           # Track specific agent PID
//   sudo ./agentcgroup -c claude,python   # Track by command filter
//   sudo ./agentcgroup -w 1              # Track specific workload ID

#include <argp.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <bpf/libbpf.h>
#include <bpf/bpf.h>
#include "agentcgroup.h"
#include "agentcgroup.skel.h"

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------
static struct env {
    int pid;
    int workload_id;
    bool verbose;
    long min_duration_ms;
    char command_filters[MAX_COMMAND_LEN];
} env = {
    .pid = 0,
    .workload_id = 0,
    .verbose = false,
    .min_duration_ms = 0,
    .command_filters = "",
};

static volatile sig_atomic_t exiting = 0;

static void sig_handler(int sig)
{
    exiting = 1;
}

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------
const char argp_program_doc[] =
    "AgentCgroup: Tool-call-aligned resource controller for AI coding agents.\n"
    "\n"
    "Tracks tool-call subprocess creation/exit from AI agent frameworks\n"
    "and outputs structured JSON events for cgroup management.\n";

static const struct argp_option opts[] = {
    { "verbose",   'v', NULL,      0, "Verbose debug output" },
    { "pid",       'p', "PID",     0, "Track specific agent PID" },
    { "workload",  'w', "ID",      0, "Track specific workload ID" },
    { "duration",  'd', "MS",      0, "Minimum tool-call duration (ms)" },
    { "comm",      'c', "CMDS",    0, "Comma-separated command filter" },
    {},
};

static error_t parse_arg(int key, char *arg, struct argp_state *state)
{
    switch (key) {
    case 'v':
        env.verbose = true;
        break;
    case 'p':
        env.pid = atoi(arg);
        break;
    case 'w':
        env.workload_id = atoi(arg);
        break;
    case 'd':
        env.min_duration_ms = atol(arg);
        break;
    case 'c':
        strncpy(env.command_filters, arg, MAX_COMMAND_LEN - 1);
        break;
    default:
        return ARGP_ERR_UNKNOWN;
    }
    return 0;
}

static const struct argp argp = {
    .options = opts,
    .parser = parse_arg,
    .doc = argp_program_doc,
};

// ---------------------------------------------------------------------------
// Tool type → string mapping
// ---------------------------------------------------------------------------
static const char *tool_type_str(enum tool_type t)
{
    switch (t) {
    case TOOL_UNKNOWN:  return "unknown";
    case TOOL_BASH:     return "bash";
    case TOOL_TEST:     return "test";
    case TOOL_BUILD:    return "build";
    case TOOL_INSTALL:  return "install";
    case TOOL_GIT:      return "git";
    case TOOL_EDITOR:   return "editor";
    case TOOL_PYTHON:   return "python";
    case TOOL_NODE:     return "node";
    case TOOL_LINT:     return "lint";
    case TOOL_FORMAT:   return "format";
    default:            return "unknown";
    }
}

// ---------------------------------------------------------------------------
// Priority → string mapping
// ---------------------------------------------------------------------------
static const char *priority_str(enum tool_priority p)
{
    switch (p) {
    case PRIORITY_LOW:    return "low";
    case PRIORITY_MEDIUM: return "medium";
    case PRIORITY_HIGH:   return "high";
    default:              return "unknown";
    }
}

// ---------------------------------------------------------------------------
// Event type → string mapping
// ---------------------------------------------------------------------------
static const char *event_type_str(enum agentcgroup_event_type t)
{
    switch (t) {
    case ACGROUP_EVENT_TOOL_START:  return "tool_start";
    case ACGROUP_EVENT_TOOL_EXIT:   return "tool_exit";
    case ACGROUP_EVENT_MEMORY_HIGH: return "memory_high";
    case ACGROUP_EVENT_MEMORY_MAX:  return "memory_max";
    case ACGROUP_EVENT_THROTTLE:    return "throttle";
    case ACGROUP_EVENT_FREEZE:      return "freeze";
    case ACGROUP_EVENT_UNFREEZE:    return "unfreeze";
    case ACGROUP_EVENT_OOM:         return "oom";
    default:                        return "unknown";
    }
}

// ---------------------------------------------------------------------------
// Degradation level → string mapping
// ---------------------------------------------------------------------------
static const char *degrade_str(enum degradation_level l)
{
    switch (l) {
    case DEGRADE_NORMAL:        return "normal";
    case DEGRADE_SOFT_THROTTLE: return "soft_throttle";
    case DEGRADE_HARD_THROTTLE: return "hard_throttle";
    case DEGRADE_FREEZE:        return "freeze";
    case DEGRADE_TERMINATE:     return "terminate";
    default:                    return "unknown";
    }
}

// ---------------------------------------------------------------------------
// JSON escaping for command strings
// ---------------------------------------------------------------------------
static void print_json_escaped(FILE *f, const char *s, int max_len)
{
    for (int i = 0; i < max_len && s[i]; i++) {
        switch (s[i]) {
        case '"':  fprintf(f, "\\\""); break;
        case '\\': fprintf(f, "\\\\"); break;
        case '\n': fprintf(f, "\\n");  break;
        case '\r': fprintf(f, "\\r");  break;
        case '\t': fprintf(f, "\\t");  break;
        case '\0': /* NUL in command args → space */
            if (i + 1 < max_len && s[i + 1])
                fprintf(f, " ");
            break;
        default:
            if ((unsigned char)s[i] >= 0x20)
                fputc(s[i], f);
            else
                fprintf(f, "\\u%04x", (unsigned char)s[i]);
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// Ring buffer event handler → JSON to stdout
// ---------------------------------------------------------------------------
static int handle_event(void *ctx, void *data, size_t data_sz)
{
    const struct agentcgroup_event *e = data;

    // Command filter (if set)
    if (env.command_filters[0] != '\0') {
        // Simple comma-separated substring matching
        bool matched = false;
        char filters[MAX_COMMAND_LEN];
        strncpy(filters, env.command_filters, MAX_COMMAND_LEN - 1);
        filters[MAX_COMMAND_LEN - 1] = '\0';

        char *token = strtok(filters, ",");
        while (token) {
            if (strstr(e->comm, token) != NULL) {
                matched = true;
                break;
            }
            token = strtok(NULL, ",");
        }
        if (!matched)
            return 0;
    }

    // Output structured JSON
    printf("{\"type\":\"%s\",\"timestamp_ns\":%llu,\"pid\":%u,\"ppid\":%u,"
           "\"workload_id\":%u,\"tool_type\":\"%s\",\"priority\":\"%s\","
           "\"exit_code\":%u,\"duration_ns\":%llu,"
           "\"memory_usage_bytes\":%llu,\"memory_limit_bytes\":%llu,"
           "\"degradation_level\":\"%s\","
           "\"comm\":\"%s\",\"full_command\":\"",
           event_type_str(e->type),
           (unsigned long long)e->timestamp_ns,
           e->pid, e->ppid,
           e->workload_id,
           tool_type_str(e->tool),
           priority_str(e->priority),
           e->exit_code,
           (unsigned long long)e->duration_ns,
           (unsigned long long)e->memory_usage_bytes,
           (unsigned long long)e->memory_limit_bytes,
           degrade_str(e->degrade_level),
           e->comm);

    print_json_escaped(stdout, e->full_command, MAX_COMMAND_LEN);
    printf("\"}\n");
    fflush(stdout);

    return 0;
}

// ---------------------------------------------------------------------------
// libbpf print callback
// ---------------------------------------------------------------------------
static int libbpf_print_fn(enum libbpf_print_level level, const char *format,
                           va_list args)
{
    if (!env.verbose && level >= LIBBPF_DEBUG)
        return 0;
    return vfprintf(stderr, format, args);
}

// ---------------------------------------------------------------------------
// Register a workload in the BPF workload_map
// Called when --pid is provided to bootstrap tracking.
// ---------------------------------------------------------------------------
static int register_workload(int map_fd, __u32 root_pid, __u32 wid)
{
    struct workload_meta meta = {};
    meta.workload_id = wid;
    meta.root_pid = root_pid;
    meta.total_memory_bytes = ((__u64)DEFAULT_TOOL_MAX_MB) << 20;
    meta.baseline_memory_bytes = ((__u64)DEFAULT_BASELINE_MEMORY_MB) << 20;
    meta.active_tool_calls = 0;
    meta.total_tool_calls = 0;

    int err = bpf_map_update_elem(map_fd, &root_pid, &meta, BPF_ANY);
    if (err) {
        fprintf(stderr, "Failed to register workload %u for PID %u: %d\n",
                wid, root_pid, err);
        return err;
    }
    return 0;
}

// ---------------------------------------------------------------------------
// Initialize default policy parameters
// ---------------------------------------------------------------------------
static int init_default_policies(int map_fd)
{
    struct policy_params policies[MAX_POLICY_ENTRIES] = {
        [PRIORITY_LOW] = {
            .memory_high_bytes = ((__u64)TOOL_MEM_HIGH_GIT_MB) << 20,
            .memory_max_bytes  = 100ULL << 20,
            .level_1_threshold = DEFAULT_LEVEL_1_THRESHOLD,
            .level_2_threshold = DEFAULT_LEVEL_2_THRESHOLD,
            .level_3_threshold = DEFAULT_LEVEL_3_THRESHOLD,
            .level_1_delay_ms  = DEFAULT_LEVEL_1_DELAY_MS,
            .level_2_delay_ms  = DEFAULT_LEVEL_2_DELAY_MS,
            .cpu_weight        = 50,
        },
        [PRIORITY_MEDIUM] = {
            .memory_high_bytes = ((__u64)DEFAULT_TOOL_HIGH_MB) << 20,
            .memory_max_bytes  = ((__u64)DEFAULT_TOOL_MAX_MB) << 20,
            .level_1_threshold = DEFAULT_LEVEL_1_THRESHOLD,
            .level_2_threshold = DEFAULT_LEVEL_2_THRESHOLD,
            .level_3_threshold = DEFAULT_LEVEL_3_THRESHOLD,
            .level_1_delay_ms  = DEFAULT_LEVEL_1_DELAY_MS,
            .level_2_delay_ms  = DEFAULT_LEVEL_2_DELAY_MS,
            .cpu_weight        = 100,
        },
        [PRIORITY_HIGH] = {
            .memory_high_bytes = ((__u64)TOOL_MEM_HIGH_TEST_MB) << 20,
            .memory_max_bytes  = ((__u64)DEFAULT_TOOL_MAX_MB) << 20,
            .level_1_threshold = DEFAULT_LEVEL_1_THRESHOLD,
            .level_2_threshold = DEFAULT_LEVEL_2_THRESHOLD,
            .level_3_threshold = DEFAULT_LEVEL_3_THRESHOLD,
            .level_1_delay_ms  = DEFAULT_LEVEL_1_DELAY_MS,
            .level_2_delay_ms  = DEFAULT_LEVEL_2_DELAY_MS,
            .cpu_weight        = 200,
        },
    };

    for (__u32 i = 0; i < MAX_POLICY_ENTRIES; i++) {
        int err = bpf_map_update_elem(map_fd, &i, &policies[i], BPF_ANY);
        if (err) {
            fprintf(stderr, "Failed to set policy for tier %u: %d\n", i, err);
            return err;
        }
    }
    return 0;
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------
int main(int argc, char **argv)
{
    struct agentcgroup_bpf *skel = NULL;
    struct ring_buffer *rb = NULL;
    int err;

    // Parse arguments
    err = argp_parse(&argp, argc, argv, 0, NULL, NULL);
    if (err)
        return err;

    // Set up libbpf
    libbpf_set_print(libbpf_print_fn);

    // Install signal handlers
    signal(SIGINT, sig_handler);
    signal(SIGTERM, sig_handler);

    // Open BPF skeleton
    skel = agentcgroup_bpf__open();
    if (!skel) {
        fprintf(stderr, "Failed to open BPF skeleton\n");
        return 1;
    }

    // Configure rodata
    skel->rodata->target_pid = env.pid;
    skel->rodata->target_workload_id = env.workload_id;
    skel->rodata->min_duration_ns = env.min_duration_ms * 1000000ULL;

    // Load and verify
    err = agentcgroup_bpf__load(skel);
    if (err) {
        fprintf(stderr, "Failed to load BPF skeleton: %d\n", err);
        goto cleanup;
    }

    // Initialize default policies
    err = init_default_policies(bpf_map__fd(skel->maps.policy_map));
    if (err)
        goto cleanup;

    // Register initial workload if PID specified
    if (env.pid) {
        __u32 wid = env.workload_id ? env.workload_id : 1;
        err = register_workload(bpf_map__fd(skel->maps.workload_map),
                                env.pid, wid);
        if (err)
            goto cleanup;

        // Also add to tracked_pids
        int tracked_fd = bpf_map__fd(skel->maps.tracked_pids);
        __u32 pid = env.pid;
        bpf_map_update_elem(tracked_fd, &pid, &wid, BPF_ANY);

        if (env.verbose)
            fprintf(stderr, "Registered workload %u for PID %u\n", wid, pid);
    }

    // Attach all BPF programs
    err = agentcgroup_bpf__attach(skel);
    if (err) {
        fprintf(stderr, "Failed to attach BPF programs: %d\n", err);
        goto cleanup;
    }

    // Create ring buffer
    rb = ring_buffer__new(bpf_map__fd(skel->maps.events), handle_event,
                          NULL, NULL);
    if (!rb) {
        err = -1;
        fprintf(stderr, "Failed to create ring buffer\n");
        goto cleanup;
    }

    if (env.verbose) {
        fprintf(stderr, "AgentCgroup tool-call tracker started\n");
        if (env.pid)
            fprintf(stderr, "  Tracking PID: %d\n", env.pid);
        if (env.workload_id)
            fprintf(stderr, "  Workload ID: %d\n", env.workload_id);
        if (env.command_filters[0])
            fprintf(stderr, "  Command filters: %s\n", env.command_filters);
        fprintf(stderr, "  Min duration: %ld ms\n", env.min_duration_ms);
    }

    // Main event loop
    while (!exiting) {
        err = ring_buffer__poll(rb, 100 /* timeout ms */);
        if (err == -EINTR) {
            err = 0;
            break;
        }
        if (err < 0) {
            fprintf(stderr, "Ring buffer poll error: %d\n", err);
            break;
        }
    }

cleanup:
    ring_buffer__free(rb);
    agentcgroup_bpf__destroy(skel);
    return err < 0 ? 1 : 0;
}
