use crate::installation as state;
use crate::installation::installation_paths::Installation;
use crate::process_identity::ProcessIdentity;
use crate::sandbox::{self, ForceStopResult};
use anyhow::{bail, Result};
use std::collections::BTreeMap;

#[derive(Debug)]
struct RunningInstance {
    app_id: String,
    instance_id: String,
    launcher_pid: libc::pid_t,
    launcher_identity: ProcessIdentity,
}

impl RunningInstance {
    fn from_record(record: &BTreeMap<String, String>) -> Option<Self> {
        let app_id = record.get("app_id")?.clone();
        let launcher_pid: libc::pid_t = record.get("launcher_pid")?.parse().ok()?;
        if launcher_pid <= 0 {
            return None;
        }
        let instance_id = record
            .get("instance_id")
            .cloned()
            .unwrap_or_else(|| launcher_pid.to_string());
        let launcher_identity = record
            .get("launcher_start")
            .and_then(|value| ProcessIdentity::parse(value))?;
        Some(Self {
            app_id,
            instance_id,
            launcher_pid,
            launcher_identity,
        })
    }

    fn matches(&self, target: &str) -> bool {
        self.app_id == target || self.instance_id == target
    }
}

trait StopRequester {
    fn request(&self, instance: &RunningInstance) -> Result<ForceStopResult>;
}

struct SystemStopRequester;

impl StopRequester for SystemStopRequester {
    fn request(&self, instance: &RunningInstance) -> Result<ForceStopResult> {
        sandbox::force_stop_launcher(instance.launcher_pid, instance.launcher_identity)
    }
}

fn parse_args(args: Vec<String>) -> Result<String> {
    match args.as_slice() {
        [target] => Ok(target.clone()),
        _ => bail!("usage: flatpak kill INSTANCE"),
    }
}

fn kill_records(
    records: Vec<BTreeMap<String, String>>,
    target: &str,
    requester: &impl StopRequester,
) -> Result<()> {
    let mut stopped = 0;
    let mut errors = Vec::new();

    for instance in records
        .iter()
        .filter_map(RunningInstance::from_record)
        .filter(|instance| instance.matches(target))
    {
        match requester.request(&instance) {
            Ok(ForceStopResult::Signaled | ForceStopResult::Exited) => stopped += 1,
            Ok(ForceStopResult::Stale) => {}
            Err(error) => errors.push(format!(
                "instance {} (launcher {}): {error:#}",
                instance.instance_id, instance.launcher_pid
            )),
        }
    }

    if !errors.is_empty() {
        bail!("failed to stop {target}: {}", errors.join("; "));
    }
    if stopped == 0 {
        bail!("{target} is not running");
    }
    Ok(())
}

pub(crate) fn cmd_kill(paths: &Installation, args: Vec<String>) -> Result<()> {
    let target = parse_args(args)?;
    kill_records(
        state::read_run_records(paths)?,
        &target,
        &SystemStopRequester,
    )
}

#[cfg(test)]
#[path = "tests/kill.rs"]
mod tests;
