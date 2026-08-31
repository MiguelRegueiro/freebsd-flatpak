use crate::installation as state;
use crate::installation::installation_paths::Installation;
use crate::process_identity::ProcessIdentity;
use crate::sandbox::{self, ForceStopResult};
use anyhow::{bail, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug)]
struct Launcher {
    pid: libc::pid_t,
    identity: ProcessIdentity,
}

#[derive(Debug)]
struct RunningInstance {
    app_id: String,
    instance_id: String,
    root: Option<PathBuf>,
    launcher: Option<Launcher>,
}

impl RunningInstance {
    fn from_record(record: &BTreeMap<String, String>) -> Option<Self> {
        let app_id = record.get("app_id")?.clone();
        let launcher_pid = record
            .get("launcher_pid")
            .and_then(|value| value.parse::<libc::pid_t>().ok())
            .filter(|pid| *pid > 0);
        let instance_id = record
            .get("instance_id")
            .cloned()
            .or_else(|| launcher_pid.map(|pid| pid.to_string()))
            .unwrap_or_else(|| app_id.clone());
        let launcher = launcher_pid.and_then(|pid| {
            record
                .get("launcher_start")
                .and_then(|value| ProcessIdentity::parse(value))
                .map(|identity| Launcher { pid, identity })
        });
        Some(Self {
            app_id,
            instance_id,
            root: record.get("root").map(PathBuf::from),
            launcher,
        })
    }

    fn matches(&self, target: &str) -> bool {
        self.app_id == target || self.instance_id == target
    }
}

trait StopRequester {
    fn request_launcher(&self, launcher: Launcher) -> Result<ForceStopResult>;
    fn terminate_roots(&self, roots: &[PathBuf]) -> Result<bool>;
}

struct SystemStopRequester;

impl StopRequester for SystemStopRequester {
    fn request_launcher(&self, launcher: Launcher) -> Result<ForceStopResult> {
        sandbox::force_stop_launcher(launcher.pid, launcher.identity)
    }

    fn terminate_roots(&self, roots: &[PathBuf]) -> Result<bool> {
        sandbox::terminate_processes_referencing_roots(roots)
    }
}

fn ownership_roots(
    instance: &RunningInstance,
    records: &[BTreeMap<String, String>],
) -> Vec<PathBuf> {
    let Some(root) = instance.root.clone() else {
        return Vec::new();
    };
    let mut roots = vec![root];
    loop {
        let mut changed = false;
        for record in records {
            let Some(parent) = record.get("parent_root").map(PathBuf::from) else {
                continue;
            };
            if roots.contains(&parent) {
                if let Some(child) = record.get("root").map(PathBuf::from) {
                    if !roots.contains(&child) {
                        roots.push(child);
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    roots
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
    processes: &sandbox::SandboxProcessSnapshot,
    requester: &impl StopRequester,
) -> Result<()> {
    let mut stopped = 0;
    let mut errors = Vec::new();

    for instance in records
        .iter()
        .filter(|record| !record.contains_key("parent_root"))
        .filter_map(RunningInstance::from_record)
        .filter(|instance| instance.matches(target))
    {
        let roots = ownership_roots(&instance, &records);
        let root_paths = roots.iter().map(PathBuf::as_path).collect::<Vec<_>>();
        let owned_processes = processes
            .pids_referencing_roots(&root_paths)
            .into_iter()
            .any(|pid| pid != std::process::id() as libc::pid_t);

        let launcher_result = if let Some(launcher) = instance.launcher {
            match requester.request_launcher(launcher) {
                Ok(result) => Some(result),
                Err(error) => {
                    errors.push(format!(
                        "instance {} (launcher {}): {error:#}",
                        instance.instance_id, launcher.pid
                    ));
                    continue;
                }
            }
        } else {
            None
        };

        match launcher_result {
            Some(ForceStopResult::Signaled) => {
                stopped += 1;
                continue;
            }
            Some(ForceStopResult::Exited) if !owned_processes => {
                stopped += 1;
                continue;
            }
            Some(ForceStopResult::Exited | ForceStopResult::Stale) | None => {}
        }

        if owned_processes {
            match requester.terminate_roots(&roots) {
                Ok(_) => stopped += 1,
                Err(error) => errors.push(format!(
                    "instance {} (owned processes): {error:#}",
                    instance.instance_id
                )),
            }
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
    let records = state::read_sandbox_ownership_records(paths)?;
    let processes = sandbox::SandboxProcessSnapshot::capture()?;
    kill_records(records, &target, &processes, &SystemStopRequester)
}

#[cfg(test)]
#[path = "tests/kill.rs"]
mod tests;
