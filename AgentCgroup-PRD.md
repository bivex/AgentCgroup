# AgentCgroup: Understanding and Controlling OS Resources of AI Agents
## Product Requirements Document

**Version:** 2.0  
**Date:** February 13, 2026  
**Status:** Research-Backed Draft  
**Based on:** arXiv:2602.09345v1 [cs.OS] 10 Feb 2026  
**Authors:** Yusheng Zheng, Jiakun Fan, Quanzhi Fu, Yiwei Yang, Wei Zhang, Andi Quinn

---

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Research Background](#research-background)
3. [Problem Statement](#problem-statement)
4. [Solution Overview](#solution-overview)
5. [Target Users & Use Cases](#target-users--use-cases)
6. [Key Features](#key-features)
7. [Technical Architecture](#technical-architecture)
8. [Functional Requirements](#functional-requirements)
9. [Non-Functional Requirements](#non-functional-requirements)
10. [Success Metrics](#success-metrics)
11. [Development Roadmap](#development-roadmap)
12. [Technical Risks & Mitigations](#technical-risks--mitigations)
13. [Competition Analysis](#competition-analysis)
14. [Go-to-Market Strategy](#go-to-market-strategy)
15. [Appendices](#appendices)

---

## Executive Summary

**AgentCgroup** is an eBPF-based resource controller designed specifically for AI coding agents (Claude Code, OpenHands, SWE-agent) that addresses critical resource management gaps in multi-tenant cloud environments. Based on comprehensive measurements of 144 software engineering tasks across two LLM models (Claude Haiku 4.5 and GLM-4.7-Flash), AgentCgroup solves three fundamental mismatches between traditional resource controls and AI agent workloads.

**Research-Validated Value Propositions:**
- **29% reduction** in high-priority P95 latency under multi-tenant memory contention
- **100% OOM survival** vs 66% baseline in tight memory conditions
- **Microsecond-level response** to resource bursts (vs millisecond-to-minute for existing solutions)
- **Graceful degradation** (throttling → freezing → termination) preserving accumulated LLM context

**Quantified Problem Scale:**
- OS-level execution accounts for **56–74%** of end-to-end task latency
- Memory exhibits **15.4× peak-to-average ratio** (vs 1.5–3× for traditional workloads)
- Resource demands vary **20× across tasks** and **1.8× across runs** of the same task
- **85–97% of tasks** contain retry loops with progressive memory accumulation

**Target Market:**
- Cloud providers hosting commercial AI agents (OpenAI, GitHub Copilot, Google Jules, Devin)
- Enterprise teams deploying coding agents internally
- AI agent frameworks (OpenHands, SWE-agent) requiring production-grade resource management

**MVP Timeline:** 3–6 months for open-source prototype with validated enforcement capabilities

---

## Research Background

### Study Methodology

The AgentCgroup project is grounded in systematic empirical research conducted on real AI coding agent workloads:

**Experimental Setup:**
- **Platform:** Intel Core Ultra 9 285K (24 cores, 128 GB DDR5), Ubuntu 24.04, Linux 6.15.11
- **Dataset:** 144 tasks from SWE-rebench benchmark
- **Models Evaluated:**
  - Claude Haiku 4.5 (cloud API): 33 tasks
  - GLM-4.7-Flash (local GPU): 111 tasks
- **Execution Environment:** Isolated Podman containers with official SWE-rebench images (2.9–17.3 GB)
- **Measurement:** 1-second sampling intervals for CPU and memory; tool-call timestamp tracking

### Key Quantitative Findings

#### 1. OS-Level Execution Dominates Task Latency
- **Tool execution + initialization: 56–74%** of end-to-end time
- LLM reasoning: Only 26–44%
- Container/agent initialization alone: 29–45% (dominated by Podman user-namespace ID remapping)
- Tool execution (active time): 36.4–42.5%

**Implication:** Traditional LLM-centric optimization misses the majority of optimization opportunities

#### 2. Memory is the Concurrency Bottleneck
- Average CPU utilization: **13.2% (Haiku)** and **7.6% (GLM)** (normalized to one core)
- Peak memory: **2–4 GB per agent**
- With 128 GB RAM: Only **32–64 concurrent instances** supported by memory (CPU utilization <36%)
- Framework baseline: Stable **~185 MB** (Node.js runtime, V8 JIT cache)
- Tool-call bursts: **500 MB – 2 GB** spikes

**Implication:** Multi-tenant density limited by memory, not CPU

#### 3. Extreme Memory Peak-to-Average Ratios
- **15.4× peak-to-average** in worst case (pydicom/pydicom#2022: 4060 MB peak, 264 MB average)
- Memory spikes last **1–2 seconds**, then fall back to ~230 MB baseline
- **98.5% of memory bursts** occur during tool calls (tool execution: 50.6% of time in Haiku)
- Change rates: Up to **3 GB/s** maximum memory change

**Comparison with Traditional Workloads:**
| Workload Type | Peak/Avg Ratio | Reference |
|---------------|----------------|-----------|
| Azure Functions | ~1.5× | Shahrad et al. 2020 |
| Azure VMs (microservices) | 2–3× | Cortez et al. 2017 |
| Google Autopilot recommendation | Within 2× | Rzadca et al. 2020 |
| **AI Coding Agents** | **15.4×** | **This study** |

#### 4. High Unpredictability
- **Cross-task variation:** 20× difference in resource demands (197 MB to 4 GB peak memory, CV=147%)
- **Cross-run variation:** 1.8× execution time variance for same task (402s, 222s, 259s across 3 runs)
- **Cross-model variation:** 1.7× CPU utilization difference (Haiku 13.2% vs GLM 7.6%)
- LLM-observable proxies are uninformative:
  - Conversation rounds: r=+0.57 to +0.82 with execution time
  - Conversation rounds: r<0.11 with peak memory

**Implication:** History-based prediction (VPA, Autopilot) fundamentally incompatible

#### 5. Tool Call Characteristics
- **Bash dominates:** 98.1% of tool time (GLM), 47.8% (Haiku)
- **Test execution dominates Bash:** 72.9% (Haiku), 43.7% (GLM) of Bash time
- **Retry loops pervasive:** 85% (Haiku) to 97% (GLM) of tasks contain ≥3 consecutive retries
- **Progressive accumulation:** Up to 502 MB unreleased memory in retry cycles
- **Temporal pattern:** Read (0–30% progress) → Bash (40–80%) → Edit (distributed)

#### 6. Container Images Are Massive
- Average size: **3.5 GB** (median), range 2.9–17.3 GB
- **7× larger** than typical microservice images (~500 MB)
- **70× larger** than serverless functions (~50 MB)
- Total dataset: 456 GB for 111 GLM tasks (114 deduplicated images)

**Implication:** Spawning new containers for finer granularity is prohibitively expensive

---

## Problem Statement

### The Core Problem

AI coding agents execute tool calls (compilers, test runners, package managers) inside sandboxed containers with fundamentally different characteristics from all known cloud workload categories:

| Dimension | Serverless/FaaS | Microservices | Batch/HPC | **AI Coding Agents** |
|-----------|-----------------|---------------|-----------|---------------------|
| **Execution duration** | 100ms–2s | Long-running | Minutes–hours | **5–11 minutes** |
| **Container image** | ~50 MB | 100 MB–1 GB | 1–10 GB | **2.9–17.3 GB (median 3.5 GB)** |
| **Statefulness** | Stateless | External state | Stateful | **In-process stateful (LLM context)** |
| **Memory footprint** | 128–512 MB | Steady ~1 GB | Scales with data | **185 MB idle, peaks 2–4 GB** |
| **Memory peak/avg** | ~1.5× | 2–3× | ~1× | **15.4×** |
| **CPU utilization** | Brief spike | 10–40% | 80–100% | **<13% avg, peaks >175%** |
| **Determinism** | Deterministic | Mostly deterministic | Deterministic | **1.8× variance for same task** |
| **Resource pattern** | Flat | Steady + daily cycle | Stable rise | **Burst-silence alternating** |
| **Termination cost** | Just retry | Can migrate | Lose progress | **Lose all LLM context** |

### Three Resource Management Mismatches

These unique characteristics create three fundamental incompatibilities with existing resource controls:

#### 1. Granularity Mismatch

**Problem:** Agent resource demands vary at **tool-call granularity** (seconds), but all existing controls set a single policy at **container level** (lifecycle).

**Temporal Dimension:**
- Container-level `memory.max` forces binary choice:
  - Set to peak (4060 MB): **Wastes 93%** of allocated memory (needed only 2% of time)
  - Set to average: Triggers **OOM kills** during 1–2 second tool bursts, destroying all accumulated agent state
- Retry loops compound this: progressive memory accumulation means adequate early limits trigger OOM by 5th iteration

**Resource Type Dimension:**
- Memory bursts concentrated in tool calls: **98.5%**
- CPU bursts more dispersed: **55.3%**
- CPU–memory correlation: **−0.84 to +0.50** across tasks
- Kubernetes QoS classes tie CPU and memory into single class, despite decoupled behavior

**Existing Controls Impact:**
| Control | Granularity | Agent Workload Result |
|---------|-------------|----------------------|
| cgroup v2 `memory.max` | Container-level | 93% waste OR frequent OOM |
| cgroup v2 `memory.high` | Container-level | GC pressure on framework baseline (185 MB) |
| Kubernetes QoS | Pod-level, CPU+memory tied | Cannot differentiate git (13.5 MB) from pytest (518 MB P95) |

#### 2. Responsiveness Mismatch

**Problem:** Agent resource bursts last **1–2 seconds** with unpredictable timing, requiring **real-time kernel-level reaction**. Existing solutions react at **millisecond-to-minute** timescales in user-space.

**Burst Characteristics:**
- Duration: 1–2 seconds
- Change rate: Up to **3 GB/s** memory, **50%/s CPU**
- Timing: Unpredictable (non-deterministic tool-call sequences)
- Frequency: **1.7–3.8%** of 1-second intervals exceed 100 MB/s change

**Existing Solutions Response Time:**
| Solution | Detection | Decision | Action | Total Latency | Agent Burst Duration |
|----------|-----------|----------|--------|---------------|---------------------|
| PSI-driven (oomd) | ~10ms (kernel PSI) | ~5ms (daemon poll) | ~5ms (cgroup write) | **~20ms** | **1–2 seconds** |
| Kubernetes VPA | N/A | Minutes (history) | Pod restart | **Minutes** | **1–2 seconds** |
| K8s in-place resize | N/A | Minutes | ~1 minute | **Minutes** | **1–2 seconds** |
| **Required** | **<1ms** | **<1ms** | **<1ms** | **<1ms** | **1–2 seconds** |

**Consequences of Slow Reaction:**
1. **Missed intervention:** Burst completes before action taken
2. **Inappropriate action:** OOM triggered unnecessarily
3. **Expensive recovery:** Container restart = 31–48% of task time (multi-GB image initialization)

#### 3. Adaptability Mismatch

**Problem:** Agent workloads are **non-deterministic**, violating history-based prediction assumptions. Traditional fallback (kill-and-restart) imposes triple penalty.

**Non-Determinism Evidence:**
- **Across tasks:** 20× resource demand variation (197 MB to 4 GB)
- **Across runs:** 1.8× execution time variance for same task (iterative/dvc#777: 402s, 222s, 259s)
- **Within execution:** Retry loops cause progressive accumulation (up to 502 MB unreleased)
- **LLM proxies uninformative:** Output token count vs peak memory r=−0.14

**History-Based Solutions Failure:**
| Solution | Assumption | Agent Reality | Result |
|----------|------------|---------------|---------|
| Kubernetes VPA | Historical P95 valid | 1.8× variance across runs | Recommendation too high or too low |
| Google Autopilot | Repeatable demand | Same task → different solution paths | Historical percentiles meaningless |
| Borg [Verma 2015] | Utilization patterns stable | Within-run accumulation | Predictions invalid |

**Triple Penalty of Kill-and-Restart:**
1. **Slow recovery:** Multi-GB agent container images make cold-start recovery consume **31–48%** of total task time (orders of magnitude longer than serverless cold starts)
2. **Lost state:** OOM kill destroys **minutes of accumulated in-process LLM context** (cannot be checkpointed or migrated like microservices with external state stores)
3. **Non-deterministic re-execution:** Re-running same task follows **entirely different solution path** (new code modifications, different strategies), so restart doesn't guarantee convergence to original solution

**Required:** Runtime adaptation based on real-time observation with graceful degradation (throttling, freezing) rather than termination.

---

## Solution Overview

### High-Level Architecture

AgentCgroup addresses the three mismatches through three corresponding innovations:

| Mismatch | Innovation | Mechanism |
|----------|-----------|-----------|
| **Granularity** | Tool-call-aligned resource domains | Hierarchical cgroup v2 structures |
| **Responsiveness** | In-kernel enforcement | eBPF hooks (sched_ext, memcg_bpf_ops) |
| **Adaptability** | Runtime-adaptive policies | In-kernel monitoring + graceful degradation |

### System Architecture Diagram

```
┌─────────────────────────────────────────────────────────┐
│                 AI Agent Framework                      │
│           (Claude Code, OpenHands, etc.)                │
│  • Reason-then-act loop                                 │
│  • Tool-use requests                                    │
│  • LLM context management                               │
└───────────────────────┬─────────────────────────────────┘
                        │ tool call execution
                        ▼
┌─────────────────────────────────────────────────────────┐
│              AgentCgroup Daemon (User Space)            │
│  • cgroup lifecycle management                          │
│  • Policy configuration via BPF maps                    │
│  • Metrics export (Prometheus/StatsD)                   │
│  • Tool-call boundary detection coordination            │
└───────────────────────┬─────────────────────────────────┘
                        │ BPF map updates
                        ▼
┌─────────────────────────────────────────────────────────┐
│                    Linux Kernel                         │
│  ┌──────────────────────────────────────────────────┐   │
│  │  sched_ext (CPU Scheduling) [Linux 6.15+]       │   │
│  │  • Per-tool-call priority enforcement           │   │
│  │  • Preemption decisions (<1ms)                  │   │
│  │  • Fail-safe reversion to CFS                   │   │
│  └──────────────────────────────────────────────────┘   │
│  ┌──────────────────────────────────────────────────┐   │
│  │  memcg_bpf_ops (Memory Control) [RFC Patches]   │   │
│  │  • Custom throttle delays (get_high_delay_ms)   │   │
│  │  • Graceful degradation policies                │   │
│  │  • Real-time pressure monitoring                │   │
│  └──────────────────────────────────────────────────┘   │
│  ┌──────────────────────────────────────────────────┐   │
│  │  eBPF Tracepoints (Process Lifecycle)           │   │
│  │  • sched_process_fork: Tool-call detection      │   │
│  │  • sched_process_exit: Cleanup + metrics        │   │
│  │  • mm events: Memory allocation tracking        │   │
│  └──────────────────────────────────────────────────┘   │
│  ┌──────────────────────────────────────────────────┐   │
│  │  eBPF Maps (Shared Kernel-User State)           │   │
│  │  • tool_call_metadata: pid, priority, timing    │   │
│  │  • policy_parameters: thresholds, delays        │   │
│  │  • metrics_counters: throttles, OOMs, bursts    │   │
│  └──────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────┘
                        │
                        ▼
┌─────────────────────────────────────────────────────────┐
│           Hierarchical cgroup v2 Structure              │
│  /sys/fs/cgroup/agentcgroup/                            │
│    ├── workload-1/              ← Agent-level quota     │
│    │    ├── baseline/           ← ~185MB framework      │
│    │    │    memory.high = 200M                         │
│    │    │    memory.max = 250M                          │
│    │    ├── tool-call-1/        ← Dynamic per-tool      │
│    │    │    memory.high = 500M  (pytest)               │
│    │    │    memory.max = 2G                            │
│    │    ├── tool-call-2/                                │
│    │    │    memory.high = 50M   (git status)           │
│    │    │    memory.max = 100M                          │
│    │    └── ...                                         │
│    ├── workload-2/                                      │
│    └── ...                                              │
└─────────────────────────────────────────────────────────┘
```

### Core Innovations Explained

#### Innovation 1: Tool-Call-Aligned Resource Domains (→ Granularity Mismatch)

**Problem Recap:** Container-level policies waste 93% of memory or trigger OOM kills

**Solution:** Hierarchical cgroup v2 structure where each tool call maps to a dedicated cgroup child node

**Implementation:**
- **Agent workload cgroup** (`/agentcgroup/workload-<id>/`):
  - Overall budget for the agent instance
  - Contains framework baseline cgroup + per-tool-call cgroups
  
- **Framework baseline cgroup** (`/agentcgroup/workload-<id>/baseline/`):
  - Dedicated 185 MB for Node.js runtime, V8 JIT cache
  - Stable allocation, protected from tool-call reclaim pressure
  
- **Per-tool-call cgroups** (`/agentcgroup/workload-<id>/tool-<id>/`):
  - Dynamic limits based on detected tool type:
    - `pytest` / test execution: `memory.high=518M` (P95), `memory.max=2G`
    - `pip install`: `memory.high=233M`, `memory.max=500M`
    - `git status`: `memory.high=14M`, `memory.max=50M`
  - Automatic creation on `sched_process_fork`, cleanup on `sched_process_exit`

**Benefits:**
- No waste: Each tool call gets only what it needs
- No OOM: Soft limits (`memory.high`) trigger throttling before hard limits
- Decoupled CPU/memory: CPU and memory controllers operate independently per tool call

#### Innovation 2: In-Kernel Enforcement (→ Responsiveness Mismatch)

**Problem Recap:** User-space controllers react at ~20ms timescale; agent bursts occur at 1–2s with unpredictable timing

**Solution:** Execute control logic directly at kernel cgroup enforcement points via eBPF

**Implementation:**

**CPU Scheduling (sched_ext):**
```c
// BPF program invoked at scheduler decision points
SEC("struct_ops/sched_ops")
void BPF_PROG(agentcgroup_enqueue, struct task_struct *p, u64 enq_flags) {
    u32 pid = p->pid;
    struct tool_call_meta *meta = bpf_map_lookup_elem(&tool_call_map, &pid);
    
    if (meta && meta->priority == PRIORITY_HIGH) {
        // Dispatch to high-priority DSQ (Dispatch Queue)
        scx_bpf_dispatch(p, SCX_DSQ_GLOBAL, SCX_SLICE_DFL, enq_flags);
    } else {
        // Normal CFS handling for low-priority
        scx_bpf_dispatch(p, SCX_DSQ_LOCAL, SCX_SLICE_DFL, enq_flags);
    }
}
```

**Memory Control (memcg_bpf_ops):**
```c
// BPF program invoked when cgroup breaches memory.high
SEC("struct_ops/memcg_ops")
u64 BPF_PROG(agentcgroup_get_high_delay_ms, struct mem_cgroup *memcg) {
    struct policy_params *policy = get_policy_for_cgroup(memcg);
    u64 current_usage = memcg->memory.usage;
    u64 high_limit = memcg->memory.high;
    
    // Calculate pressure ratio
    u64 pressure = (current_usage * 100) / high_limit;
    
    // Graduated response based on pressure
    if (pressure < policy->level_1_threshold) {
        return 0;  // Normal operation
    } else if (pressure < policy->level_2_threshold) {
        return policy->level_1_delay_ms;  // Throttle (50-100ms)
    } else if (pressure < policy->level_3_threshold) {
        // Freeze cgroup via cgroup.freeze (handled in user-space coordination)
        bpf_send_signal_to_daemon(memcg->id, ACTION_FREEZE);
        return 500;  // Heavy throttle while freeze propagates
    } else {
        // Last resort: allow OOM but with graceful cleanup
        bpf_send_signal_to_daemon(memcg->id, ACTION_KILL);
        return 1000;
    }
}
```

**Reaction Time Comparison:**
| Component | Latency | Mechanism |
|-----------|---------|-----------|
| **Pressure detection** | ~0.1ms | Kernel memory controller event |
| **BPF program execution** | ~0.5ms | In-kernel BPF verifier-approved code |
| **Policy decision** | ~0.1ms | BPF map lookup |
| **Enforcement** | ~0.3ms | Kernel cgroup file write |
| **Total AgentCgroup** | **~1ms** | **End-to-end in-kernel** |
| Traditional user-space | ~20ms | Kernel→user→decision→kernel round-trip |

#### Innovation 3: Runtime-Adaptive Policies (→ Adaptability Mismatch)

**Problem Recap:** History-based prediction fails due to 20× cross-task and 1.8× cross-run variance; kill-restart destroys LLM context

**Solution:** Real-time monitoring with graceful degradation, no reliance on historical data

**Implementation:**

**Degradation Ladder (per tool-call cgroup):**
```
Level 0: Normal Operation
  ├─ Pressure < 70% of memory.high
  ├─ Action: None
  └─ Delay: 0ms

Level 1: Soft Throttle
  ├─ Pressure 70–90% of memory.high
  ├─ Action: Increase allocation latency
  └─ Delay: 50ms per allocation

Level 2: Hard Throttle
  ├─ Pressure 90–95% of memory.high
  ├─ Action: Heavy reclaim pressure
  └─ Delay: 100ms per allocation

Level 3: Freeze
  ├─ Pressure 95–99% of memory.high
  ├─ Action: cgroup.freeze (pause all processes in subtree)
  └─ Wait: Until high-priority workloads release memory

Level 4: Terminate (Last Resort)
  ├─ Pressure > memory.max
  ├─ Action: cgroup.kill (atomic termination of subtree)
  └─ Preserve: Framework baseline cgroup (LLM context in parent)
```

**Key Characteristics:**
- **No historical prediction:** Policies react to current pressure only
- **Graceful degradation:** Multiple intermediate steps before termination
- **Context preservation:** Freeze instead of kill preserves accumulated LLM state
- **Automatic recovery:** Unfreeze when pressure drops below threshold

**Monitoring Inputs (collected in-kernel via eBPF):**
- Real-time memory usage per cgroup (`memory.current`)
- Memory allocation rate (tracked via `mm_page_alloc` tracepoint)
- Process creation rate (tool-call frequency detection)
- Retry loop detection (consecutive identical Bash commands)

---

## Target Users & Use Cases
│  │  memcg_bpf_ops (Memory Control)       │    │
│  │  • Throttling delays                  │    │
│  │  • Graceful degradation policies       │    │
│  └─────────────────────────────────────────────┘    │
│  ┌─────────────────────────────────────────────┐    │
│  │  eBPF Maps (Shared State)             │    │
│  │  • Tool-call metadata                │    │
│  │  • Policy parameters                  │    │
│  │  • Metrics counters                  │    │
│  └─────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────┘
                        │
                        ▼
┌─────────────────────────────────────────────────────────┐
│           Hierarchical cgroup v2 Structure            │
│  /agentcgroup/                                     │
│    ├── workload-1/         ← Agent-level quota     │
│    │    ├── baseline/      ← ~185MB framework      │
│    │    ├── tool-call-1/   ← Dynamic limits      │
│    │    ├── tool-call-2/                          │
│    │    └── ...                                 │
│    ├── workload-2/                                 │
│    └── ...                                       │
└─────────────────────────────────────────────────────────┘
```

### Core Innovations

1. **Tool-Call-Aligned cgroups** — Each subprocess gets its own resource domain
2. **In-Kernel Enforcement** — Microsecond reaction via sched_ext and memcg_bpf_ops
3. **Graceful Degradation** — Throttling → Freezing → Termination (not kill-first)
4. **Runtime-Adaptive Policies** — Real-time monitoring, not historical prediction

---

## Target Users & Use Cases

### Primary Users

| User Type | Pain Points | Desired Outcome |
|-----------|--------------|------------------|
| **Cloud Platform Engineers** | Memory costs, unpredictable agent workloads, OOM kills | Higher density, predictable costs |
| **AI Agent Framework Developers** | Resource management is external, not integrated | First-class resource control as feature |
| **DevOps/SRE Teams** | Debugging agent failures, scaling decisions | Visibility + automated control |

### Secondary Users

| User Type | Use Case |
|-----------|-----------|
| **Enterprise IT** | Internal AI agent deployments with SLA guarantees |
| **Researchers** | Studying AI agent behavior with fine-grained metrics |
| **Infrastructure Vendors** | Building agent-optimized offerings |

### Use Cases

1. **Multi-Tenant Cloud Hosting**
   - Scenario: Hosting 100+ concurrent AI coding agents
   - Before: 128GB RAM = 32–64 agents, frequent OOM
   - After: 100+ agents, 0% OOM kills, 29% P95 latency reduction

2. **Enterprise Internal Deployment**
   - Scenario: 20 developers using Claude Code on-premises
   - Before: Manual resource allocation, poor isolation
   - After: Automatic per-user isolation, no OOM disruption

3. **Agent Framework Integration**
   - Scenario: OpenHands embedding resource control
   - Before: Relies on K8s defaults, poor performance
   - After: Built-in AgentCgroup support, optimized execution

---

## Key Features

### MVP Features (v0.1)

| Feature | Description | Priority |
|---------|-------------|-----------|
| **Tool-Call Tracking** | eBPF process creation hooks to detect tool-call boundaries | P0 |
| **Hierarchical cgroups** | Automatic cgroup v2 structure aligned with tool calls | P0 |
| **Memory Throttling** | memcg_bpf_ops custom delays on memory.high breaches | P0 |
| **CPU Scheduling** | sched_ext-based per-tool-call priorities | P1 |
| **Policy Engine** | Simple rules (priority thresholds, degradation levels) | P1 |
| **Metrics Export** | Prometheus endpoint for monitoring | P1 |
| **CLI Tool** | Simple command-line interface for management | P2 |

### v0.2 Features

| Feature | Description |
|---------|-------------|
| **Policy Templates** | Pre-defined profiles for different agent frameworks |
| **Dynamic Policies** | Runtime policy updates via REST API |
| **Web Dashboard** | Real-time visualization of resource usage |
| **Alerting** | Prometheus AlertManager integration |

### Future Features (v1.0+)

| Feature | Description |
|---------|-------------|
| **ML-Based Prediction** | Learn from agent behavior to pre-allocate resources |
| **Cross-Agent Coordination** | Global resource optimization across workloads |
| **Kubernetes Operator** | Native K8s integration |
| **GPU Support** | eBPF-based GPU resource tracking |

---

## Technical Architecture

### Components

#### 1. eBPF Programs (Kernel Space)

**CPU Scheduling (sched_ext)**
```c
// Per-tool-call metadata stored in BPF map
struct tool_call_meta {
    __u32 pid;
    __u32 priority;  // HIGH, MEDIUM, LOW
    __u64 start_time;
    __u64 cpu_time;
};

// Hook into scheduler
SEC("struct_ops/sched_ops")
int sched_tool_call_dispatch(struct task_struct *p) {
    // Read priority from BPF map
    // Apply scheduling decisions
    // Update metrics
    return 0;
}
```

**Memory Control (memcg_bpf_ops)**
```c
// Custom throttling delay
SEC("struct_ops/memcg_ops")
int get_high_delay_ms(struct mem_cgroup *memcg) {
    // Check degradation level
    // Return delay: 0ms (normal) → 100ms (throttle) → freeze
}

// Graceful degradation policy
if (pressure > HIGH_THRESHOLD) {
    return 100ms;  // Throttle
} else if (pressure > CRITICAL_THRESHOLD) {
    return FREEZE;  // cgroup.freeze
}
```

**Tool-Call Tracking**
```c
// Trace process creation
SEC("tracepoint/sched/sched_process_fork")
int trace_tool_call_start(void *ctx, struct task_struct *parent) {
    // Detect if subprocess belongs to agent
    // Create child cgroup
    // Initialize metadata
    return 0;
}

SEC("tracepoint/sched/sched_process_exit")
int trace_tool_call_end(void *ctx) {
    // Record metrics
    // Clean up cgroup
    // Notify user-space daemon
    return 0;
}
```

#### 2. User-Space Daemon (Rust)

**Responsibilities:**
```rust
struct AgentCgroupDaemon {
    // cgroup lifecycle
    fn create_workload_cgroup(agent_id: &str) -> Result<Cgroup>;
    fn create_tool_call_cgroup(workload_id: &str, call_id: u64) -> Result<Cgroup>;

    // Policy management
    fn update_policy(&self, policy: Policy) -> Result<()>;
    fn apply_degradation_level(&self, cgroup: &Cgroup, level: DegradationLevel);

    // Metrics export
    fn export_metrics(&self) -> Result<()>;

    // eBPF map communication
    fn sync_bpf_maps(&self) -> Result<()>;
}
```

#### 3. CLI Tool

```bash
# Start daemon
sudo agentcgroup daemon --config /etc/agentcgroup/config.yaml

# Monitor specific agent
sudo agentcgroup monitor --agent-id <id>

# Apply policy
sudo agentcgroup policy apply --agent-id <id> --priority HIGH

# Query metrics
sudo agentcgroup metrics --agent-id <id> --format prometheus
```

### Data Flow

```
1. Agent executes tool call
       ↓
2. eBPF tracepoint detects subprocess
       ↓
3. eBPF creates child cgroup with metadata
       ↓
4. Tool runs under cgroup limits
       ↓
5. Memory pressure detected (eBPF hook)
       ↓
6. memcg_bpf_ops applies throttling delay
       ↓
7. Process exits → eBPF updates metrics
       ↓
8. User-space daemon syncs BPF maps
       ↓
9. Metrics exported to Prometheus
```

---

## Functional Requirements

### FR-001: Tool-Call Detection
- **Requirement:** Automatically detect when an AI agent spawns a subprocess for a tool call
- **Priority:** P0
- **Acceptance Criteria:**
  - Detect process creation from known agent frameworks (Claude Code, OpenHands)
  - Identify tool type (bash, compiler, test runner)
  - Create dedicated cgroup for each subprocess

### FR-002: Hierarchical cgroup Management
- **Requirement:** Create and manage hierarchical cgroup v2 structure aligned with workloads and tool calls
- **Priority:** P0
- **Acceptance Criteria:**
  - Top-level `/agentcgroup/` cgroup
  - Workload-level cgroups (`/agentcgroup/workload-<id>/`)
  - Tool-call cgroups (`/agentcgroup/workload-<id>/tool-<id>/`)
  - Automatic cleanup on process exit

### FR-003: Memory Throttling
- **Requirement:** Apply graceful memory throttling using memcg_bpf_ops
- **Priority:** P0
- **Acceptance Criteria:**
  - Custom delay function hooked to memory.high
  - Three degradation levels: NORMAL (0ms), THROTTLE (50-100ms), FREEZE
  - Automatic transition based on pressure metrics

### FR-004: CPU Scheduling
- **Requirement:** Implement priority-based CPU scheduling via sched_ext
- **Priority:** P1
- **Acceptance Criteria:**
  - HIGH priority tasks preempt LOW/MEDIUM
  - Per-tool-call metadata stored in BPF map
  - Preemption decisions within 1ms

### FR-005: Policy Engine
- **Requirement:** Simple policy configuration for resource limits and degradation
- **Priority:** P1
- **Acceptance Criteria:**
  - YAML configuration file
  - Per-workload memory.high and memory.max
  - Priority tiers (HIGH, MEDIUM, LOW)
  - Degradation thresholds

### FR-006: Metrics Export
- **Requirement:** Export resource metrics in Prometheus format
- **Priority:** P1
- **Acceptance Criteria:**
  - HTTP endpoint `/metrics`
  - Metrics: memory usage, CPU utilization, throttling events, OOM kills
  - Labels: agent_id, tool_type, priority

### FR-007: CLI Management
- **Requirement:** Provide command-line interface for management
- **Priority:** P2
- **Acceptance Criteria:**
  - Subcommands: `daemon`, `monitor`, `policy`, `metrics`
  - Real-time monitoring output
  - Policy apply/query commands

### FR-008: Graceful Degradation
- **Requirement:** Degrade performance gradually instead of OOM kill
- **Priority:** P0
- **Acceptance Criteria:**
  - Level 1: Throttling (memory.high delay)
  - Level 2: Freeze (cgroup.freeze)
  - Level 3: Kill (memory.oom.group, last resort)
  - Automatic progression based on pressure

### FR-009: Kernel Patch Compatibility
- **Requirement:** Support memcg_bpf_ops and sched_ext kernel patches
- **Priority:** P0
- **Acceptance Criteria:**
  - Compile against patched kernel headers
  - Detect available features at runtime
  - Graceful fallback if patches not present

### FR-010: Multi-Agent Support
- **Requirement:** Support multiple concurrent agent workloads
- **Priority:** P1
- **Acceptance Criteria:**
  - Independent cgroup hierarchies per agent
  - Global resource management across workloads
  - No interference between agents

---

## Non-Functional Requirements

### NFR-001: Performance Overhead
- **Requirement:** AgentCgroup overhead must be <5% of baseline
- **Measurement:** CPU time, memory overhead, latency impact
- **Target:** <3% (based on paper's evaluation)

### NFR-002: Response Latency
- **Requirement:** Response to resource events within 1ms
- **Measurement:** Time from pressure detection to policy enforcement
- **Target:** Microsecond-level (eBPF in-kernel)

### NFR-003: Reliability
- **Requirement:** 99.9% uptime for daemon, no kernel panics
- **Measurement:** Crash rate, error logs
- **Target:** Fail-safe fallback to default behavior

### NFR-004: Scalability
- **Requirement:** Support 100+ concurrent tool calls
- **Measurement:** cgroup count, BPF map entries
- **Target:** Linear performance scaling

### NFR-005: Security
- **Requirement:** No privilege escalation, secure BPF map access
- **Measurement:** Security audit
- **Target:** CAP_BPF + CAP_SYS_ADMIN only, no other capabilities

### NFR-006: Compatibility
- **Requirement:** Support Linux kernel 5.0+ with required patches
- **Measurement:** Tested kernel versions
- **Target:** Ubuntu 20.04+, RHEL 8+, Debian 11+

### NFR-007: Observability
- **Requirement:** Full logging and metrics for debugging
- **Measurement:** Log coverage, metric completeness
- **Target:** Structured logs with TRACE/WARN/ERROR levels

---

## Success Metrics

### Technical Metrics

| Metric | Target | Measurement Method |
|---------|--------|-------------------|
| **OOM Survival Rate** | ≥95% | Replay of agent traces under memory pressure |
| **P95 Latency Reduction** | ≥25% | High-priority task completion time |
| **Performance Overhead** | <3% | CPU utilization with/without AgentCgroup |
| **Response Latency** | <1ms | Time from pressure detection to enforcement |
| **Kernel Crash Rate** | 0 | Stability testing over 1000+ hours |

### User Metrics

| Metric | Target | Measurement Method |
|---------|--------|-------------------|
| **Setup Time** | <10 minutes | Time from installation to first successful run |
| **CLI Usability** | ≤3 commands for common tasks | User survey, task completion rate |
| **Documentation Coverage** | 100% of features documented | Docstring coverage, tutorial completeness |

### Adoption Metrics

| Metric | Target (6 months) |
|---------|-------------------|
| **GitHub Stars** | 500+ |
| **Forks** | 50+ |
| **Issues Resolved** | 90% within 7 days |
| **Contributors** | 10+ |
| **Production Deployments** | 5+ (cloud providers or enterprise) |

---

## Development Roadmap

### Phase 1: MVP (Months 1-3)

**Goal:** Basic tool-call tracking and memory throttling

| Week | Milestone | Deliverables |
|------|-----------|--------------|
| 1-2 | Project Setup | Repo structure, CI/CD, libbpf-rs integration |
| 3-4 | Tool-Call Tracking | eBPF process creation hooks, cgroup creation |
| 5-6 | Memory Control | memcg_bpf_ops throttling implementation |
| 7-8 | Policy Engine | YAML config, policy application |
| 9-10 | Metrics Export | Prometheus endpoint, basic metrics |
| 11-12 | CLI Tool | Subcommands, monitoring, policy apply |

**Success Criteria:**
- Detect and control tool calls for Claude Code
- Memory throttling with 3 degradation levels
- Metrics export functional

### Phase 2: CPU Scheduling (Months 4-5)

**Goal:** Priority-based CPU scheduling via sched_ext

| Week | Milestone | Deliverables |
|------|-----------|--------------|
| 1-2 | sched_ext Integration | Per-tool-call priorities, preemption logic |
| 3-4 | CPU Metrics | CPU utilization per tool call |
| 5-6 | Testing | Workload benchmarks, latency measurement |

**Success Criteria:**
- HIGH priority tasks preempt LOW priority
- <1ms preemption latency
- CPU metrics exported

### Phase 3: Evaluation & Benchmarking (Month 6)

**Goal:** Validate against paper's results

| Week | Milestone | Deliverables |
|------|-----------|--------------|
| 1-2 | Trace Replay | Replay SWE-bench traces, collect metrics |
| 3-4 | Comparison | Baseline vs AgentCgroup comparison |
| 5-6 | Analysis | Write evaluation paper, optimize based on findings |

**Success Criteria:**
- ≥90% OOM survival
- ≥25% P95 latency reduction
- <3% overhead

### Phase 4: Production Readiness (Months 7-9)

**Goal:** Enterprise features and stability

| Feature | Description |
|---------|-------------|
| **Web Dashboard** | Real-time visualization of cgroups and metrics |
| **Policy Templates** | Pre-configured profiles for Claude Code, OpenHands, SWE-agent |
| **Dynamic Policies** | REST API for runtime policy updates |
| **Alerting** | Prometheus AlertManager integration |
| **Logging** | Structured logs with rotation, error tracking |

### Phase 5: Kubernetes Integration (Months 10-12)

**Goal:** Native K8s support

| Feature | Description |
|---------|-------------|
| **Operator** | Deploy AgentCgroup DaemonSet automatically |
| **CRDs** | AgentCgroupPolicy, AgentCgroupMetrics |
| **Admission Controller** | Auto-inject AgentCgroup into agent pods |
| **Horizontal Scaling** | Auto-scale DaemonSet based on cluster size |

---

## Technical Risks & Mitigations

### Risk 1: Kernel Patch Availability

**Risk:** memcg_bpf_ops and sched_ext patches not upstream

**Probability:** Medium
**Impact:** High

**Mitigation:**
1. Build against patched kernel (document requirement)
2. Provide build instructions for specific kernel versions
3. Work with upstream maintainers for merge
4. Fallback: Use user-space throttling if patches unavailable

### Risk 2: eBPF Verifier Rejection

**Risk:** eBPF programs rejected by verifier due to complexity

**Probability:** Low
**Impact:** Medium

**Mitigation:**
1. Keep BPF programs simple (avoid complex loops)
2. Use BPF CO-RE for kernel compatibility
3. Extensive unit testing across kernel versions
4. Fallback to user-space policies

### Risk 3: Tool-Call Detection Accuracy

**Risk:** False positives/negatives in detecting agent tool calls

**Probability:** Medium
**Impact:** Medium

**Mitigation:**
1. Use multiple heuristics (parent process, command line, environment)
2. Allow user configuration of agent binaries
3. Learning mode: observe and classify
4. Manual override capability

### Risk 4: Performance Regression

**Risk:** AgentCgroup overhead >5%

**Probability:** Low
**Impact:** High

**Mitigation:**
1. Profile continuously with perf/BPF
2. Optimize hot paths (in-kernel critical)
3. Minimize BPF map lookups
4. Benchmark against baseline regularly

### Risk 5: cgroup Race Conditions

**Risk:** Concurrent tool-call creation causing cgroup conflicts

**Probability:** Low
**Impact:** Medium

**Mitigation:**
1. Atomic cgroup operations via eBPF
2. Lock-free BPF maps
3. Fuzz testing for race conditions
4. Idempotent operations (retry on conflict)

---

## Competition Analysis

### Direct Competitors

| Solution | Status | Strengths | Weaknesses |
|-----------|--------|------------|-------------|
| **Kubernetes VPA/QoS** | Production | Native K8s, widely used | Pod-level only, history-based |
| **systemd-oomd** | Production | PSI-driven, mature | Kill-only, no throttling |
| **Meta oomd** | Production | Advanced policies | Facebook-optimized, not agent-aware |
| **AgentSight** | Open Source | eBPF monitoring, zero-instrumentation | Observability only, no enforcement |

### Indirect Competitors

| Solution | Category | Relevance |
|-----------|-----------|------------|
| **Trigger.dev** | Orchestration | Workflow management, not resource control |
| **Julep.ai** | Orchestration | Serverless, standard cloud scaling |
| **BentoML OpenLLM** | Serving | LLM inference, not tool-call resources |
| **Datadog/New Relic** | APM | Application-level, can be bypassed |

### Competitive Advantages

1. **Tool-Call-Level Granularity** — Unique to AgentCgroup
2. **eBPF In-Kernel Enforcement** — Microsecond response time
3. **Graceful Degradation** — No competitor offers throttling/freezing
4. **Agent Workload Awareness** — Optimized specifically for AI agents
5. **Open Source** — Lower barrier to adoption vs enterprise tools

---

## Go-to-Market Strategy

### Phase 1: Open Source MVP (Months 1-3)

**Objective:** Validate technical feasibility and build community

**Tactics:**
- Publish open-source repository under MIT license
- Write blog post announcing AgentCgroup
- Submit talk to KubeCon/eBPF Summit
- Target AI agent frameworks (OpenHands, SWE-agent) for integration

**Success Metrics:**
- 100+ GitHub stars
- 10+ community contributors
- 2 framework integrations announced

### Phase 2: Enterprise Beta (Months 4-9)

**Objective:** Production deployments with paying customers

**Tactics:**
- Contact cloud providers (AWS, GCP, Azure) for partnership
- Offer enterprise features (SaaS, support, SLAs)
- Create case studies with early adopters
- Develop K8s operator for easy deployment

**Success Metrics:**
- 5 production deployments
- 3 paying customers
- 1 major cloud provider integration

### Phase 3: Commercial Launch (Months 10-12)

**Objective:** Full commercial product

**Offerings:**
- **SaaS:** $0.01/agent-hour
- **Enterprise:** $10k/year/node, includes support
- **Community:** Free open-source version with basic features

**Target Markets:**
1. **Cloud Platforms** — AWS Bedrock, Azure AI, Google Cloud AI
2. **Enterprise IT** — Companies with >100 AI agents
3. **Agent Frameworks** — OpenHands, SWE-agent integration

**Pricing Strategy:**
- Freemium model: Open-source free, Enterprise paid
- Value-based pricing: Charge on memory savings achieved
- Volume discounts for large deployments

---

## Appendices

### Appendix A: Configuration Example

```yaml
# /etc/agentcgroup/config.yaml
version: "0.1"

workloads:
  - id: "claude-code-1"
    priority: HIGH
    memory:
      baseline: "185M"    # Framework overhead
      tool_high: "500M"    # Soft limit per tool call
      tool_max: "2G"       # Hard limit per tool call
    degradation:
      level_1_threshold: 0.7   # 70% of tool_high
      level_1_delay: 50ms         # Throttle
      level_2_threshold: 0.9   # 90% of tool_high
      level_2_action: "freeze"   # Freeze cgroup
      level_3_threshold: 0.95   # 95% of tool_high
      level_3_action: "kill"     # Last resort
    agent_binary: "/usr/bin/claude-code"
    enabled_tools: ["bash", "python", "npm", "cargo"]

global:
  metrics_port: 9090
  metrics_path: "/metrics"
  log_level: "INFO"
  log_file: "/var/log/agentcgroup/daemon.log"
```

### Appendix B: eBPF Map Structures

```c
// Tool-call metadata map
struct bpf_map_def SEC("maps") tool_call_map = {
    .type = BPF_MAP_TYPE_HASH,
    .key_size = sizeof(__u32),  // PID
    .value_size = sizeof(struct tool_call_meta),
    .max_entries = 10240,
};

// Policy parameters map
struct bpf_map_def SEC("maps") policy_map = {
    .type = BPF_MAP_TYPE_ARRAY,
    .key_size = sizeof(__u32),  // Priority tier
    .value_size = sizeof(struct policy_params),
    .max_entries = 3,  // HIGH, MEDIUM, LOW
};

// Metrics counters
struct bpf_map_def SEC("maps") metrics_map = {
    .type = BPF_MAP_TYPE_PERCPU_ARRAY,
    .key_size = sizeof(__u32),
    .value_size = sizeof(__u64),
    .max_entries = 32,  // Various metric types
};
```

### Appendix C: Integration with AgentSight

```
┌─────────────────────────────────────────────────────────────┐
│                  AgentSight (Observability)             │
│  • SSL/TLS traffic interception                        │
│  • Process tree visualization                          │
│  • Timeline and event logs                            │
└─────────────────────────┬───────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────────────┐
│                  AgentCgroup (Enforcement)             │
│  • Tool-call resource control                          │
│  • Graceful degradation                               │
│  • Policy enforcement                                 │
└─────────────────────────┬───────────────────────────────┘
                      │
                      ▼
              AI Agent (Claude Code, OpenHands)
```

**Synergy:**
- AgentSight provides visibility → AgentCgroup enforces control
- Metrics from both tools correlated for comprehensive picture
- Alerting: AgentSight detects → AgentCgroup responds

### Appendix D: References

1. **AgentCgroup Paper:** https://arxiv.org/abs/2602.09345
2. **Linux cgroup v2 Documentation:** https://docs.kernel.org/admin-guide/cgroup-v2.html
3. **sched_ext Documentation:** https://docs.kernel.org/scheduler/sched-ext.html
4. **eBPF Documentation:** https://ebpf.io/
5. **AgentSight:** https://github.com/eunomia-bpf/agentsight
6. **OpenHands:** https://github.com/OpenHands/openhands
7. **SWE-agent:** https://github.com/princeton-nlp/SWE-agent
8. **Claude Code:** https://code.claude.com

---

## Document History

| Version | Date | Author | Changes |
|---------|-------|--------|----------|
| 1.0 | 2026-02-12 | Initial PRD for AgentCgroup prototype |
