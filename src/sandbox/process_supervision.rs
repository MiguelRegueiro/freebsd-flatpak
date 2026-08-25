use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

pub(super) fn wait_for_sandbox_process_group(
    root: &Path,
    process_group: i32,
    termination_requested: impl FnMut() -> bool,
) -> Result<()> {
    // A process that deliberately leaves the launch group is detached from the
    // app lifecycle. Chroot cleanup will terminate any such remaining process.
    wait_while_processes_remain(
        || sandbox_process_group_alive(root, process_group),
        termination_requested,
        || thread::sleep(Duration::from_millis(100)),
    )
}

fn sandbox_process_group_alive(root: &Path, process_group: i32) -> Result<bool> {
    let output = Command::new("ps")
        .args(["-axo", "pid,pgid"])
        .output()
        .context("list sandbox process groups")?;
    if !output.status.success() {
        bail!("ps -axo pid,pgid failed with status {}", output.status);
    }
    let text = String::from_utf8(output.stdout)?;
    for pid in process_group_pids(&text, process_group) {
        if pid != std::process::id() as i32 && process_rooted_in(pid, root)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn process_group_pids(processes: &str, process_group: i32) -> impl Iterator<Item = i32> + '_ {
    processes.lines().filter_map(move |line| {
        let mut fields = line.split_whitespace();
        let pid = fields.next()?.parse::<i32>().ok()?;
        let pgid = fields.next()?.parse::<i32>().ok()?;
        (pgid == process_group).then_some(pid)
    })
}

fn wait_while_processes_remain(
    mut processes_remain: impl FnMut() -> Result<bool>,
    mut termination_requested: impl FnMut() -> bool,
    mut pause: impl FnMut(),
) -> Result<()> {
    while !termination_requested() && processes_remain()? {
        pause();
    }
    Ok(())
}

pub(super) fn process_rooted_in(pid: i32, root: &Path) -> Result<bool> {
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
