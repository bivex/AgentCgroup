// AgentCgroup: cgroup v2 hierarchy manager
//
// Manages the cgroup v2 filesystem structure for AgentCgroup:
//
//   /sys/fs/cgroup/agentcgroup/
//     ├── workload-{id}/                ← Agent-level quota
//     │   ├── baseline/                 ← Framework baseline (~185 MB)
//     │   ├── tool-{pid}/               ← Per-tool-call cgroup
//     │   └── ...
//     └── ...
//
// Operations are all synchronous filesystem writes — cgroup v2 is controlled
// entirely via the pseudo-filesystem at /sys/fs/cgroup.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use log::{debug, info, warn, error};

use super::types::*;
use super::policy::WorkloadPolicy;

// ---------------------------------------------------------------------------
// CgroupManager: owns the hierarchy lifecycle
// ---------------------------------------------------------------------------

pub struct CgroupManager {
    /// Root path: /sys/fs/cgroup/agentcgroup
    root: PathBuf,
    /// Active workloads: workload_id → WorkloadInfo
    workloads: HashMap<u32, WorkloadInfo>,
    /// Active tool calls: pid → ToolCallInfo
    tool_calls: HashMap<u32, ToolCallInfo>,
}

impl CgroupManager {
    /// Create a new CgroupManager and initialize the root cgroup.
    pub fn new() -> Result<Self, CgroupError> {
        let root = PathBuf::from(CGROUP_ROOT).join(AGENTCGROUP_ROOT);

        let mgr = CgroupManager {
            root,
            workloads: HashMap::new(),
            tool_calls: HashMap::new(),
        };

        Ok(mgr)
    }

    /// Initialize the root cgroup hierarchy on the filesystem.
    /// Must be called with root privileges.
    pub fn init(&self) -> Result<(), CgroupError> {
        // Create root: /sys/fs/cgroup/agentcgroup/
        ensure_dir(&self.root)?;

        // Enable memory and cpu controllers in subtree
        Self::enable_controllers(&self.root)?;

        info!("AgentCgroup root initialized at {}", self.root.display());
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Workload lifecycle
    // -----------------------------------------------------------------------

    /// Register a new workload (agent instance) and create its cgroup hierarchy.
    pub fn create_workload(
        &mut self,
        workload_id: u32,
        root_pid: u32,
        agent_binary: &str,
        policy: &WorkloadPolicy,
    ) -> Result<&WorkloadInfo, CgroupError> {
        let wl_path = self.root.join(format!("workload-{}", workload_id));

        // Create workload cgroup
        ensure_dir(&wl_path)?;
        Self::enable_controllers(&wl_path)?;

        // Create baseline cgroup for framework overhead
        let baseline_path = wl_path.join("baseline");
        ensure_dir(&baseline_path)?;

        // Apply baseline memory limits
        let baseline_high = policy.baseline_memory_mb * 1024 * 1024;
        let baseline_max = (policy.baseline_memory_mb + 65) * 1024 * 1024; // +65 MB headroom
        write_cgroup_file(&baseline_path, "memory.high", &baseline_high.to_string())?;
        write_cgroup_file(&baseline_path, "memory.max", &baseline_max.to_string())?;

        // Apply workload-level limits (total budget)
        let total_max = policy.total_memory_max_mb * 1024 * 1024;
        write_cgroup_file(&wl_path, "memory.max", &total_max.to_string())?;

        // Move root PID into the baseline cgroup
        if root_pid > 0 {
            write_cgroup_file(&baseline_path, "cgroup.procs", &root_pid.to_string())?;
        }

        let info = WorkloadInfo {
            id: workload_id,
            root_pid,
            agent_binary: agent_binary.to_string(),
            cgroup_path: wl_path.to_string_lossy().to_string(),
            priority: policy.default_priority,
            active_tool_calls: 0,
            total_tool_calls: 0,
            current_degradation: DegradationLevel::Normal,
        };

        self.workloads.insert(workload_id, info);

        info!(
            "Created workload-{} at {} (root_pid={}, baseline={}MB)",
            workload_id,
            wl_path.display(),
            root_pid,
            policy.baseline_memory_mb
        );

        Ok(self.workloads.get(&workload_id).unwrap())
    }

    /// Remove a workload and all its child cgroups.
    pub fn destroy_workload(&mut self, workload_id: u32) -> Result<(), CgroupError> {
        // First clean up all tool calls belonging to this workload
        let tool_pids: Vec<u32> = self
            .tool_calls
            .iter()
            .filter(|(_, tc)| tc.workload_id == workload_id)
            .map(|(pid, _)| *pid)
            .collect();

        for pid in tool_pids {
            if let Err(e) = self.destroy_tool_call(pid) {
                warn!("Failed to clean up tool call PID {} during workload destroy: {}", pid, e);
            }
        }

        let wl_path = self.root.join(format!("workload-{}", workload_id));

        // Remove baseline cgroup
        let baseline_path = wl_path.join("baseline");
        remove_cgroup_dir(&baseline_path)?;

        // Remove workload cgroup
        remove_cgroup_dir(&wl_path)?;

        self.workloads.remove(&workload_id);

        info!("Destroyed workload-{}", workload_id);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Tool-call lifecycle
    // -----------------------------------------------------------------------

    /// Create a cgroup for a new tool call and move the process into it.
    pub fn create_tool_call(
        &mut self,
        pid: u32,
        ppid: u32,
        workload_id: u32,
        tool_type: ToolType,
        priority: Priority,
    ) -> Result<&ToolCallInfo, CgroupError> {
        let wl_path = self.root.join(format!("workload-{}", workload_id));
        if !wl_path.exists() {
            return Err(CgroupError::NotFound(format!(
                "Workload {} not found",
                workload_id
            )));
        }

        let tc_path = wl_path.join(format!("tool-{}", pid));
        ensure_dir(&tc_path)?;

        // Apply tool-type-specific memory limits
        let limits = ToolMemoryLimits::for_tool(tool_type);
        write_cgroup_file(&tc_path, "memory.high", &limits.memory_high_bytes().to_string())?;
        write_cgroup_file(&tc_path, "memory.max", &limits.memory_max_bytes().to_string())?;

        // Set CPU weight based on priority
        let cpu_weight = match priority {
            Priority::Low => 50,
            Priority::Medium => 100,
            Priority::High => 200,
        };
        write_cgroup_file(&tc_path, "cpu.weight", &cpu_weight.to_string())?;

        // Move process into the tool-call cgroup
        write_cgroup_file(&tc_path, "cgroup.procs", &pid.to_string())?;

        let info = ToolCallInfo {
            pid,
            ppid,
            workload_id,
            tool_type,
            priority,
            cgroup_path: tc_path.to_string_lossy().to_string(),
            start_time_ns: 0, // Populated from BPF event
            memory_current_bytes: 0,
            memory_high_bytes: limits.memory_high_bytes(),
            memory_max_bytes: limits.memory_max_bytes(),
            degradation: DegradationLevel::Normal,
        };

        self.tool_calls.insert(pid, info);

        // Update workload counters
        if let Some(wl) = self.workloads.get_mut(&workload_id) {
            wl.active_tool_calls += 1;
            wl.total_tool_calls += 1;
        }

        debug!(
            "Created tool-{} in workload-{} (type={}, priority={}, mem_high={}MB, mem_max={}MB)",
            pid,
            workload_id,
            tool_type,
            priority,
            limits.memory_high_mb,
            limits.memory_max_mb
        );

        Ok(self.tool_calls.get(&pid).unwrap())
    }

    /// Destroy a tool-call cgroup after the process exits.
    pub fn destroy_tool_call(&mut self, pid: u32) -> Result<(), CgroupError> {
        if let Some(tc) = self.tool_calls.remove(&pid) {
            let tc_path = PathBuf::from(&tc.cgroup_path);
            remove_cgroup_dir(&tc_path)?;

            // Update workload counters
            if let Some(wl) = self.workloads.get_mut(&tc.workload_id) {
                wl.active_tool_calls = wl.active_tool_calls.saturating_sub(1);
            }

            debug!("Destroyed tool-{} (workload-{})", pid, tc.workload_id);
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Graceful degradation
    // -----------------------------------------------------------------------

    /// Apply a degradation level to a tool-call cgroup.
    pub fn apply_degradation(
        &mut self,
        pid: u32,
        level: DegradationLevel,
    ) -> Result<(), CgroupError> {
        let tc = self.tool_calls.get_mut(&pid).ok_or_else(|| {
            CgroupError::NotFound(format!("Tool call PID {} not found", pid))
        })?;

        let tc_path = PathBuf::from(&tc.cgroup_path);

        match level {
            DegradationLevel::Normal => {
                // Unfreeze if previously frozen
                if tc.degradation >= DegradationLevel::Freeze {
                    write_cgroup_file(&tc_path, "cgroup.freeze", "0")?;
                }
            }
            DegradationLevel::SoftThrottle | DegradationLevel::HardThrottle => {
                // Throttling is handled by reducing memory.high dynamically
                // to trigger kernel-side reclaim pressure
                let reduction = match level {
                    DegradationLevel::SoftThrottle => 0.85, // 85% of original
                    DegradationLevel::HardThrottle => 0.70, // 70% of original
                    _ => 1.0,
                };
                let new_high = (tc.memory_high_bytes as f64 * reduction) as u64;
                write_cgroup_file(&tc_path, "memory.high", &new_high.to_string())?;
            }
            DegradationLevel::Freeze => {
                // Freeze all processes in the cgroup subtree
                write_cgroup_file(&tc_path, "cgroup.freeze", "1")?;
                info!("Froze tool-{} (workload-{})", pid, tc.workload_id);
            }
            DegradationLevel::Terminate => {
                // Last resort: kill all processes in the cgroup
                write_cgroup_file(&tc_path, "cgroup.kill", "1")?;
                warn!("Terminated tool-{} (workload-{})", pid, tc.workload_id);
            }
        }

        tc.degradation = level;
        Ok(())
    }

    /// Check memory pressure for a tool call and determine degradation level.
    pub fn check_pressure(&self, pid: u32) -> Result<(DegradationLevel, u64, u64), CgroupError> {
        let tc = self.tool_calls.get(&pid).ok_or_else(|| {
            CgroupError::NotFound(format!("Tool call PID {} not found", pid))
        })?;

        let tc_path = PathBuf::from(&tc.cgroup_path);
        let current = read_cgroup_u64(&tc_path, "memory.current")?;

        let pressure_pct = if tc.memory_high_bytes > 0 {
            (current * 100) / tc.memory_high_bytes
        } else {
            0
        };

        let level = if pressure_pct < DEFAULT_LEVEL_1_THRESHOLD as u64 {
            DegradationLevel::Normal
        } else if pressure_pct < DEFAULT_LEVEL_2_THRESHOLD as u64 {
            DegradationLevel::SoftThrottle
        } else if pressure_pct < DEFAULT_LEVEL_3_THRESHOLD as u64 {
            DegradationLevel::HardThrottle
        } else if pressure_pct < 99 {
            DegradationLevel::Freeze
        } else {
            DegradationLevel::Terminate
        };

        Ok((level, current, tc.memory_high_bytes))
    }

    // -----------------------------------------------------------------------
    // Query methods
    // -----------------------------------------------------------------------

    pub fn workloads(&self) -> &HashMap<u32, WorkloadInfo> {
        &self.workloads
    }

    pub fn tool_calls(&self) -> &HashMap<u32, ToolCallInfo> {
        &self.tool_calls
    }

    pub fn get_workload(&self, id: u32) -> Option<&WorkloadInfo> {
        self.workloads.get(&id)
    }

    pub fn get_tool_call(&self, pid: u32) -> Option<&ToolCallInfo> {
        self.tool_calls.get(&pid)
    }

    /// Read current memory usage for a tool-call cgroup.
    pub fn read_memory_current(&self, pid: u32) -> Result<u64, CgroupError> {
        let tc = self.tool_calls.get(&pid).ok_or_else(|| {
            CgroupError::NotFound(format!("Tool call PID {} not found", pid))
        })?;
        let tc_path = PathBuf::from(&tc.cgroup_path);
        read_cgroup_u64(&tc_path, "memory.current")
    }

    /// Read memory stats for a workload-level cgroup.
    pub fn read_workload_memory(&self, workload_id: u32) -> Result<u64, CgroupError> {
        let wl_path = self.root.join(format!("workload-{}", workload_id));
        read_cgroup_u64(&wl_path, "memory.current")
    }

    pub fn root_path(&self) -> &Path {
        &self.root
    }

    // -----------------------------------------------------------------------
    // Cleanup
    // -----------------------------------------------------------------------

    /// Remove the entire AgentCgroup hierarchy (for shutdown).
    pub fn cleanup(&mut self) -> Result<(), CgroupError> {
        // Destroy all tool calls
        let pids: Vec<u32> = self.tool_calls.keys().copied().collect();
        for pid in pids {
            let _ = self.destroy_tool_call(pid);
        }

        // Destroy all workloads
        let wids: Vec<u32> = self.workloads.keys().copied().collect();
        for wid in wids {
            let _ = self.destroy_workload(wid);
        }

        // Remove root
        remove_cgroup_dir(&self.root)?;

        info!("AgentCgroup hierarchy cleaned up");
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Enable memory and cpu controllers in a cgroup's subtree_control.
    fn enable_controllers(path: &Path) -> Result<(), CgroupError> {
        // Read available controllers
        let available = read_cgroup_string(path, "cgroup.controllers")?;
        let mut to_enable = Vec::new();

        if available.contains("memory") {
            to_enable.push("+memory");
        }
        if available.contains("cpu") {
            to_enable.push("+cpu");
        }
        if available.contains("io") {
            to_enable.push("+io");
        }

        if !to_enable.is_empty() {
            let value = to_enable.join(" ");
            write_cgroup_file(path, "cgroup.subtree_control", &value)?;
            debug!("Enabled controllers [{}] at {}", value, path.display());
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Filesystem helpers
// ---------------------------------------------------------------------------

fn ensure_dir(path: &Path) -> Result<(), CgroupError> {
    if !path.exists() {
        fs::create_dir_all(path).map_err(|e| {
            error!("Failed to create directory {}: {}", path.display(), e);
            CgroupError::from(e)
        })?;
    }
    Ok(())
}

fn remove_cgroup_dir(path: &Path) -> Result<(), CgroupError> {
    if path.exists() {
        // cgroup directories can only be removed with rmdir (not rm -rf)
        // and must be empty (no child cgroups and no processes).
        // Try to remove, ignore errors for non-empty dirs.
        match fs::remove_dir(path) {
            Ok(()) => {
                debug!("Removed cgroup {}", path.display());
            }
            Err(e) => {
                // ENOTEMPTY or EBUSY is expected if processes are still running
                debug!("Could not remove cgroup {} (may still be in use): {}", path.display(), e);
            }
        }
    }
    Ok(())
}

fn write_cgroup_file(cgroup: &Path, file: &str, value: &str) -> Result<(), CgroupError> {
    let path = cgroup.join(file);
    fs::write(&path, value).map_err(|e| {
        debug!("Failed to write '{}' to {}: {}", value, path.display(), e);
        CgroupError::from(e)
    })
}

fn read_cgroup_string(cgroup: &Path, file: &str) -> Result<String, CgroupError> {
    let path = cgroup.join(file);
    let content = fs::read_to_string(&path).map_err(|e| {
        debug!("Failed to read {}: {}", path.display(), e);
        CgroupError::from(e)
    })?;
    Ok(content.trim().to_string())
}

fn read_cgroup_u64(cgroup: &Path, file: &str) -> Result<u64, CgroupError> {
    let s = read_cgroup_string(cgroup, file)?;
    s.parse::<u64>().map_err(|e| {
        CgroupError::InvalidState(format!(
            "Failed to parse {} as u64 from {}: {}",
            s,
            cgroup.join(file).display(),
            e
        ))
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_memory_limits() {
        let test_limits = ToolMemoryLimits::for_tool(ToolType::Test);
        assert_eq!(test_limits.memory_high_mb, 518);
        assert_eq!(test_limits.memory_max_mb, 2048);
        assert_eq!(test_limits.memory_high_bytes(), 518 * 1024 * 1024);

        let git_limits = ToolMemoryLimits::for_tool(ToolType::Git);
        assert_eq!(git_limits.memory_high_mb, 14);
        assert_eq!(git_limits.memory_max_mb, 50);
    }

    #[test]
    fn test_degradation_ordering() {
        assert!(DegradationLevel::Normal < DegradationLevel::SoftThrottle);
        assert!(DegradationLevel::SoftThrottle < DegradationLevel::HardThrottle);
        assert!(DegradationLevel::HardThrottle < DegradationLevel::Freeze);
        assert!(DegradationLevel::Freeze < DegradationLevel::Terminate);
    }
}
