# AgentCgroup Prototype - Product Requirements Document

**Version:** 1.0
**Date:** 2026-02-12
**Status:** Draft
**Author:** Generated for Roman Smolyakov

---

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Problem Statement](#problem-statement)
3. [Solution Overview](#solution-overview)
4. [Target Users & Use Cases](#target-users--use-cases)
5. [Key Features](#key-features)
6. [Technical Architecture](#technical-architecture)
7. [Functional Requirements](#functional-requirements)
8. [Non-Functional Requirements](#non-functional-requirements)
9. [Success Metrics](#success-metrics)
10. [Development Roadmap](#development-roadmap)
11. [Technical Risks & Mitigations](#technical-risks--mitigations)
12. [Competition Analysis](#competition-analysis)
13. [Go-to-Market Strategy](#go-to-market-strategy)
14. [Appendices](#appendices)

---

## Executive Summary

**AgentCgroup** is an eBPF-based resource controller designed specifically for AI coding agents (Claude Code, OpenHands, SWE-agent). It addresses the gap between generic container resource controls (Kubernetes, cgroups) and the unique, highly dynamic workloads of AI agents running tool calls in multi-tenant cloud environments.

**Key Value Propositions:**
- 29% reduction in high-priority task latency
- 100% OOM survival vs 66% baseline
- Sub-millisecond response to resource bursts
- Graceful degradation (throttling/freezing) vs kill-restart

**Target Market:**
- Cloud providers hosting AI agents (OpenAI, GitHub, Google, Cognition)
- Enterprise teams deploying coding agents internally
- AI agent frameworks (OpenHands, SWE-agent) seeking production-grade resource management

**MVP Timeline:** 3–6 months for open-source prototype with basic enforcement capabilities.

---

## Problem Statement

### The Core Problem

AI coding agents execute tool calls (compilers, test runners, package managers) inside sandboxed containers. Unlike traditional workloads (serverless, microservices, batch), AI agents exhibit:

1. **Tool-call-level resource dynamics** — Memory spikes 15.4× peak-to-average within 1–2 seconds
2. **High unpredictability** — 20× variation across tasks, 1.8× across runs
3. **Stateful execution** — Minutes of accumulated LLM context lost on OOM kills
4. **Retry loops** — Progressive memory accumulation (85–97% of tasks)

### Why Current Solutions Fail

| Solution | Granularity | Responsiveness | Adaptability |
|-----------|--------------|----------------|---------------|
| **Kubernetes VPA/QoS** | Pod-level | Minute-level | History-based prediction |
| **systemd-oomd** | Container-level | PSI-driven (ms-sec) | Kill-only, no adaptation |
| **cgroup v2 (vanilla)** | Container-level | No automated response | Static limits |
| **Containerd/Kubelet** | Container-level | No enforcement | No awareness |

**Result:** Either massive resource waste (93% allocation unused) or frequent OOM kills with 31–48% recovery overhead.

---

## Solution Overview

### High-Level Concept

AgentCgroup introduces tool-call-aligned resource domains with kernel-level enforcement via eBPF:

```
┌─────────────────────────────────────────────────────────┐
│                 AI Agent Framework                    │
│           (Claude Code, OpenHands, etc.)           │
└───────────────────────┬─────────────────────────────┘
                        │
                        ▼
┌─────────────────────────────────────────────────────────┐
│              AgentCgroup Daemon (User Space)         │
│  • cgroup lifecycle management                       │
│  • Policy configuration (BPF maps)                 │
│  • Metrics export (Prometheus/StatsD)               │
└───────────────────────┬─────────────────────────────┘
                        │
                        ▼
┌─────────────────────────────────────────────────────────┐
│                    Linux Kernel                      │
│  ┌─────────────────────────────────────────────┐    │
│  │  sched_ext (CPU Scheduling)              │    │
│  │  • Per-tool-call priorities              │    │
│  │  • Preemption decisions                 │    │
│  └─────────────────────────────────────────────┘    │
│  ┌─────────────────────────────────────────────┐    │
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
