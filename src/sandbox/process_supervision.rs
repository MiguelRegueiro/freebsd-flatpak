use anyhow::{bail, Context, Result};
use std::process::{Child, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const TERMINATION_GRACE: Duration = Duration::from_secs(2);

pub(super) struct ProcessReaper {
    acquired: bool,
}

impl ProcessReaper {
    pub(super) fn acquire() -> Result<Self> {
        set_reaper_status(true)?;
        Ok(Self { acquired: true })
    }

    pub(super) fn track(mut self, subtree: u32) -> Result<SandboxProcessTree> {
        let subtree = i32::try_from(subtree).context("sandbox process id exceeds pid_t")?;
        self.acquired = false;
        Ok(SandboxProcessTree { subtree })
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

impl SandboxProcessTree {
    pub(super) fn wait_for_exit(
        &self,
        child: &mut Child,
        mut termination_requested: impl FnMut() -> bool,
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

            if termination.is_none() && termination_requested() {
                termination = Some((libc::SIGTERM, Instant::now() + TERMINATION_GRACE));
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
        if self.wait_until_empty(TERMINATION_GRACE, libc::SIGTERM)? {
            return Ok(());
        }
        if self.wait_until_empty(TERMINATION_GRACE, libc::SIGKILL)? {
            return Ok(());
        }

        bail!(
            "sandbox process tree rooted at pid {} survived SIGKILL",
            self.subtree
        )
    }

    fn wait_until_empty(&self, timeout: Duration, signal: libc::c_int) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            if !self.processes_remain(true)? {
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
            return Err(std::io::Error::last_os_error())
                .context("list sandbox process descendants");
        }

        Ok(info
            .into_iter()
            .take_while(|process| process.flags & REAPER_PIDINFO_VALID != 0)
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
        let _ = set_reaper_status(false);
    }
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

pub(super) fn process_rooted_in(pid: i32, root: &std::path::Path) -> Result<bool> {
    use std::path::Path;
    use std::process::Command;

    let output = Command::new("procstat")
        .arg("-f")
        .arg(pid.to_string())
        .output()
        .with_context(|| format!("inspect process {pid} root"))?;
    if !output.status.success() {
        return Ok(false);
    }
    let text = String::from_utf8(output.stdout)?;
    Ok(text.lines().any(|line| {
        let mut fields = line.split_whitespace();
        let _pid = fields.next();
        let _comm = fields.next();
        let Some(fd) = fields.next() else {
            return false;
        };
        if fd != "root" && fd != "jail" && fd != "cwd" {
            return false;
        }
        fields.last().is_some_and(|path| Path::new(path) == root)
    }))
}

#[cfg(test)]
#[path = "tests/process_supervision.rs"]
mod tests;
