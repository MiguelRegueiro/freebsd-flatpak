use anyhow::{Context, Result};
use std::fmt;
use std::mem::{size_of, MaybeUninit};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ProcessIdentity {
    start_seconds: i64,
    start_microseconds: i64,
}

impl ProcessIdentity {
    pub(crate) fn for_pid(pid: libc::pid_t) -> Result<Option<Self>> {
        if pid <= 0 {
            return Ok(None);
        }

        let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
        let mut process = MaybeUninit::<libc::kinfo_proc>::zeroed();
        let mut length = size_of::<libc::kinfo_proc>();
        let result = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                mib.len() as u32,
                process.as_mut_ptr().cast(),
                &mut length,
                std::ptr::null_mut(),
                0,
            )
        };
        if result == -1 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(None);
            }
            return Err(error).with_context(|| format!("read identity for process {pid}"));
        }
        if length == 0 {
            return Ok(None);
        }
        let process = unsafe { process.assume_init() };
        if process.ki_pid != pid {
            return Ok(None);
        }

        Ok(Some(Self {
            start_seconds: process.ki_start.tv_sec,
            start_microseconds: process.ki_start.tv_usec,
        }))
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        let (seconds, microseconds) = value.split_once(':')?;
        Some(Self {
            start_seconds: seconds.parse().ok()?,
            start_microseconds: microseconds.parse().ok()?,
        })
    }
}

impl fmt::Display for ProcessIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}",
            self.start_seconds, self.start_microseconds
        )
    }
}

#[cfg(test)]
#[path = "tests/process_identity.rs"]
mod tests;
