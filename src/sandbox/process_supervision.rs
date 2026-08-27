use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

pub(super) fn wait_for_sandbox_processes(
    root: &Path,
    process_group: i32,
    termination_requested: impl FnMut() -> bool,
) -> Result<()> {
    // The launch group covers doas/chroot before chroot(2) changes the process
    // root. Once inside, the unique instance root is the durable identity:
    // applications may reparent, change process group, or create a new session.
    wait_while_processes_remain(
        || sandbox_processes_alive(root, process_group),
        termination_requested,
        || thread::sleep(Duration::from_millis(100)),
    )
}

fn sandbox_processes_alive(root: &Path, process_group: i32) -> Result<bool> {
    if launch_process_group_alive(process_group)? {
        return Ok(true);
    }

    let output = Command::new("procstat")
        .args(["-f", "-a"])
        .output()
        .context("list process roots for sandbox supervision")?;
    if !output.status.success() {
        bail!("procstat -f -a failed with status {}", output.status);
    }
    let text = String::from_utf8(output.stdout)?;
    let alive = sandbox_root_pids(&text, root).any(|pid| pid != std::process::id() as i32);
    Ok(alive)
}

fn launch_process_group_alive(process_group: i32) -> Result<bool> {
    let output = Command::new("ps")
        .args(["-axo", "pid,pgid"])
        .output()
        .context("list sandbox process groups")?;
    if !output.status.success() {
        bail!("ps -axo pid,pgid failed with status {}", output.status);
    }
    let text = String::from_utf8(output.stdout)?;
    for pid in process_group_pids(&text, process_group) {
        if pid != std::process::id() as i32 {
            return Ok(true);
        }
    }
    Ok(false)
}

fn sandbox_root_pids<'a>(processes: &'a str, root: &'a Path) -> impl Iterator<Item = i32> + 'a {
    processes.lines().filter_map(move |line| {
        let mut fields = line.split_whitespace();
        let pid = fields.next()?.parse::<i32>().ok()?;
        let _command = fields.next()?;
        let fd = fields.next()?;
        (fd == "root" && fields.last().is_some_and(|path| Path::new(path) == root)).then_some(pid)
    })
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
