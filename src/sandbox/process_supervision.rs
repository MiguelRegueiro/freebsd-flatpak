use super::process_signals::FORCE_STOP_SIGNAL;
use crate::process_identity::ProcessIdentity;
use anyhow::{bail, Context, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const TERMINATION_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub(super) struct ProcessReaper {
    acquired: bool,
}

impl ProcessReaper {
    pub(super) fn acquire() -> Result<Self> {
        set_reaper_status(true)?;
        Ok(Self { acquired: true })
    }

    #[cfg(test)]
    pub(super) fn test_inert() -> Self {
        Self { acquired: false }
    }

    pub(super) fn track(&self, subtree: u32) -> Result<SandboxProcessTree> {
        let subtree = i32::try_from(subtree).context("sandbox process id exceeds pid_t")?;
        Ok(SandboxProcessTree { subtree })
    }

    pub(super) fn subtree_for_descendant(&self, descendant: u32) -> Result<Option<SandboxSubtree>> {
        let descendant =
            i32::try_from(descendant).context("sandbox descendant id exceeds pid_t")?;
        Ok(reaper_pid_info()?
            .into_iter()
            .find(|process| process.pid == descendant)
            .map(|process| SandboxSubtree(process.subtree)))
    }

    pub(super) fn terminate_orphaned_subtree_with_signal(
        &self,
        subtree: SandboxSubtree,
        signal: libc::c_int,
    ) -> Result<bool> {
        if reaper_pid_info()?
            .into_iter()
            .any(|process| process.pid == subtree.0)
        {
            return Ok(false);
        }
        let tree = std::mem::ManuallyDrop::new(SandboxProcessTree { subtree: subtree.0 });
        tree.terminate_with_signal_mode(signal, false)?;
        Ok(true)
    }
}

impl Drop for ProcessReaper {
    fn drop(&mut self) {
        if self.acquired {
            let _ = set_reaper_status(false);
        }
    }
}

pub(super) struct SandboxProcessTree {
    subtree: libc::pid_t,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SandboxSubtree(libc::pid_t);

impl SandboxProcessTree {
    pub(super) fn wait_for_exit(
        &self,
        child: &mut Child,
        mut termination_requested: impl FnMut() -> bool,
    ) -> Result<ExitStatus> {
        self.wait_for_exit_with_signal(child, || termination_requested().then_some(libc::SIGTERM))
    }

    pub(super) fn wait_for_exit_with_signal(
        &self,
        child: &mut Child,
        mut termination_signal: impl FnMut() -> Option<libc::c_int>,
    ) -> Result<ExitStatus> {
        let mut status = None;
        let mut termination = None;

        loop {
            if status.is_none() {
                status = child.try_wait().context("wait for app process")?;
            }
            if !self.processes_remain(false)? {
                return status.context("sandbox process tree exited without app status");
            }

            if termination.is_none() {
                termination =
                    termination_signal().map(|signal| (signal, Instant::now() + TERMINATION_GRACE));
            }
            if let Some((signal, deadline)) = termination {
                self.signal(signal)?;
                if Instant::now() >= deadline {
                    if signal == libc::SIGKILL {
                        bail!(
                            "sandbox process tree rooted at pid {} survived SIGKILL",
                            self.subtree
                        );
                    }
                    termination = Some((libc::SIGKILL, Instant::now() + TERMINATION_GRACE));
                }
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    fn terminate(&self) -> Result<()> {
        self.terminate_with_signal(libc::SIGTERM)
    }

    fn terminate_with_signal(&self, signal: libc::c_int) -> Result<()> {
        self.terminate_with_signal_mode(signal, true)
    }

    fn terminate_with_signal_mode(
        &self,
        signal: libc::c_int,
        reap_subtree_root: bool,
    ) -> Result<()> {
        if self.wait_until_empty(TERMINATION_GRACE, signal, reap_subtree_root)? {
            return Ok(());
        }
        if self.wait_until_empty(TERMINATION_GRACE, libc::SIGKILL, reap_subtree_root)? {
            return Ok(());
        }

        bail!(
            "sandbox process tree rooted at pid {} survived SIGKILL",
            self.subtree
        )
    }

    fn wait_until_empty(
        &self,
        timeout: Duration,
        signal: libc::c_int,
        reap_subtree_root: bool,
    ) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            if !self.processes_remain(reap_subtree_root)? {
                return Ok(true);
            }
            self.signal(signal)?;
            if Instant::now() >= deadline {
                return Ok(false);
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    fn processes_remain(&self, reap_tracked_child: bool) -> Result<bool> {
        let subtree_pids = self.subtree_pids()?;

        for pid in &subtree_pids {
            if !reap_tracked_child && *pid == self.subtree {
                continue;
            }
            let mut status = 0;
            unsafe {
                libc::waitpid(*pid, &mut status, libc::WNOHANG);
            }
        }

        if subtree_pids.is_empty() {
            return Ok(false);
        }

        Ok(!self.subtree_pids()?.is_empty())
    }

    fn subtree_pids(&self) -> Result<Vec<libc::pid_t>> {
        Ok(reaper_pid_info()?
            .into_iter()
            .filter(|process| process.subtree == self.subtree)
            .map(|process| process.pid)
            .collect())
    }

    fn signal(&self, signal: libc::c_int) -> Result<()> {
        let mut request = ReaperKill {
            signal,
            flags: REAPER_KILL_SUBTREE,
            subtree: self.subtree,
            ..ReaperKill::default()
        };
        let result = unsafe {
            libc::procctl(
                libc::P_PID,
                0,
                libc::PROC_REAP_KILL,
                (&mut request as *mut ReaperKill).cast(),
            )
        };
        if result == -1 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error).context("signal sandbox process descendants");
            }
        }
        Ok(())
    }
}

impl Drop for SandboxProcessTree {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ForceStopResult {
    Signaled,
    Exited,
    Stale,
}

pub(crate) fn force_stop_launcher(
    pid: libc::pid_t,
    expected: ProcessIdentity,
) -> Result<ForceStopResult> {
    if ProcessIdentity::for_pid(pid)? != Some(expected) {
        return Ok(ForceStopResult::Stale);
    }
    let mut status = ReaperStatus::default();
    let result = unsafe {
        libc::procctl(
            libc::P_PID,
            pid.into(),
            libc::PROC_REAP_STATUS,
            (&mut status as *mut ReaperStatus).cast(),
        )
    };
    if result == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(ForceStopResult::Exited);
        }
        return Err(error).with_context(|| format!("inspect launcher process {pid}"));
    }
    const REAPER_STATUS_OWNED: u32 = 0x0000_0001;
    if status.flags & REAPER_STATUS_OWNED == 0 {
        return Ok(ForceStopResult::Stale);
    }
    // Narrow the identity/status inspection window before delivering the signal.
    if ProcessIdentity::for_pid(pid)? != Some(expected) {
        return Ok(ForceStopResult::Exited);
    }

    let result = unsafe { libc::kill(pid, FORCE_STOP_SIGNAL) };
    if result == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(ForceStopResult::Exited);
        }
        return Err(error).with_context(|| format!("request force-stop from launcher {pid}"));
    }
    Ok(ForceStopResult::Signaled)
}

fn set_reaper_status(acquire: bool) -> Result<()> {
    let command = if acquire {
        libc::PROC_REAP_ACQUIRE
    } else {
        libc::PROC_REAP_RELEASE
    };
    let result = unsafe { libc::procctl(libc::P_PID, 0, command, std::ptr::null_mut()) };
    if result == -1 {
        return Err(std::io::Error::last_os_error()).context(if acquire {
            "acquire sandbox process reaper"
        } else {
            "release sandbox process reaper"
        });
    }
    Ok(())
}

fn reaper_status() -> Result<ReaperStatus> {
    let mut status = ReaperStatus::default();
    let result = unsafe {
        libc::procctl(
            libc::P_PID,
            0,
            libc::PROC_REAP_STATUS,
            (&mut status as *mut ReaperStatus).cast(),
        )
    };
    if result == -1 {
        return Err(std::io::Error::last_os_error()).context("read sandbox process reaper status");
    }
    Ok(status)
}

fn reaper_pid_info() -> Result<Vec<ReaperPidInfo>> {
    let descendant_count = reaper_status()?.descendants as usize;
    if descendant_count == 0 {
        return Ok(Vec::new());
    }

    let mut info = vec![ReaperPidInfo::default(); descendant_count + 16];
    let mut request = ReaperPids {
        count: info.len() as u32,
        pad: [0; 15],
        pids: info.as_mut_ptr(),
    };
    let result = unsafe {
        libc::procctl(
            libc::P_PID,
            0,
            libc::PROC_REAP_GETPIDS,
            (&mut request as *mut ReaperPids).cast(),
        )
    };
    if result == -1 {
        return Err(std::io::Error::last_os_error()).context("list sandbox process descendants");
    }

    Ok(info
        .into_iter()
        .take_while(|process| process.flags & REAPER_PIDINFO_VALID != 0)
        .collect())
}

const REAPER_PIDINFO_VALID: u32 = 0x0000_0001;
const REAPER_KILL_SUBTREE: u32 = 0x0000_0002;

#[repr(C)]
#[derive(Default)]
struct ReaperStatus {
    flags: u32,
    children: u32,
    descendants: u32,
    reaper: libc::pid_t,
    pid: libc::pid_t,
    pad: [u32; 15],
}

#[repr(C)]
#[derive(Clone, Default)]
struct ReaperPidInfo {
    pid: libc::pid_t,
    subtree: libc::pid_t,
    flags: u32,
    pad: [u32; 15],
}

#[repr(C)]
struct ReaperPids {
    count: u32,
    pad: [u32; 15],
    pids: *mut ReaperPidInfo,
}

#[repr(C)]
#[derive(Default)]
struct ReaperKill {
    signal: libc::c_int,
    flags: u32,
    subtree: libc::pid_t,
    killed: u32,
    failed_pid: libc::pid_t,
    pad: [u32; 15],
}

#[derive(Debug, Default)]
pub(crate) struct SandboxProcessSnapshot {
    references: Vec<(libc::pid_t, PathBuf)>,
}

impl SandboxProcessSnapshot {
    pub(crate) fn capture() -> Result<Self> {
        let output = std::process::Command::new("procstat")
            .args(["-a", "-f"])
            .output()
            .context("inspect sandbox process roots")?;
        if !output.status.success() {
            bail!("procstat -a -f failed with status {}", output.status);
        }
        let text = String::from_utf8(output.stdout)?;
        Ok(Self::parse(&text))
    }

    fn parse(text: &str) -> Self {
        let mut lines = text.lines();
        let Some(layout) = lines.next().and_then(ProcstatLayout::from_header) else {
            return Self::default();
        };
        let references = lines
            .filter_map(|line| layout.reference(line))
            .filter(|(_, fd, _)| matches!(*fd, "cwd" | "root" | "jail"))
            .map(|(pid, _, path)| (pid, path))
            .collect();
        Self { references }
    }

    #[cfg(test)]
    pub(crate) fn for_test(references: Vec<(libc::pid_t, PathBuf)>) -> Self {
        Self { references }
    }

    pub(crate) fn references_root(&self, root: &Path) -> bool {
        self.references
            .iter()
            .any(|(_, path)| path.starts_with(root))
    }

    pub(crate) fn pids_referencing_root(&self, root: &Path) -> Vec<libc::pid_t> {
        self.pids_referencing_roots(std::slice::from_ref(&root))
    }

    pub(crate) fn pids_referencing_roots(&self, roots: &[&Path]) -> Vec<libc::pid_t> {
        self.references
            .iter()
            .filter(|(_, path)| roots.iter().any(|root| path.starts_with(root)))
            .map(|(pid, _)| *pid)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

pub(crate) fn terminate_processes_referencing_roots(roots: &[PathBuf]) -> Result<bool> {
    let root_paths = roots.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let initial = SandboxProcessSnapshot::capture()?.pids_referencing_roots(&root_paths);
    if initial.is_empty() {
        return Ok(false);
    }

    for signal in [libc::SIGTERM, libc::SIGKILL] {
        for _ in 0..20 {
            let pids = SandboxProcessSnapshot::capture()?
                .pids_referencing_roots(&root_paths)
                .into_iter()
                .filter(|pid| *pid != std::process::id() as libc::pid_t)
                .collect::<Vec<_>>();
            if pids.is_empty() {
                return Ok(true);
            }
            for pid in pids {
                // Every signal is preceded by a fresh kernel vnode snapshot;
                // recorded launcher and child PIDs are never used here.
                unsafe {
                    libc::kill(pid, signal);
                }
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    let remaining = SandboxProcessSnapshot::capture()?.pids_referencing_roots(&root_paths);
    if remaining.is_empty() {
        Ok(true)
    } else {
        bail!("sandbox processes survived SIGKILL: {remaining:?}")
    }
}

#[derive(Clone, Copy, Debug)]
struct ProcstatLayout {
    pid_end: usize,
    fd_start: usize,
    fd_end: usize,
    name_start: usize,
}

impl ProcstatLayout {
    fn from_header(header: &str) -> Option<Self> {
        let pid_end = header.find(" COMM")?;
        let fd_end = header.find(" T V FLAGS")?;
        let fd_start = fd_end.checked_sub(4)?;
        let name_start = header.find("NAME")?;
        Some(Self {
            pid_end,
            fd_start,
            fd_end,
            name_start,
        })
    }

    fn reference<'a>(&self, line: &'a str) -> Option<(libc::pid_t, &'a str, PathBuf)> {
        let pid = line.get(..self.pid_end)?.trim().parse().ok()?;
        let fd = line.get(self.fd_start..self.fd_end)?.trim();
        let path = PathBuf::from(line.get(self.name_start..)?.trim_end());
        Some((pid, fd, path))
    }
}

#[cfg(test)]
#[path = "tests/process_supervision.rs"]
mod tests;
