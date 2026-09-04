use super::application_entrypoint::sandbox_name;
use super::process_supervision::SandboxProcessSnapshot;
use crate::installation as state;
use crate::installation::installation_paths::Installation;
use crate::secure_mount;
use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

pub fn recover_stale_mounts(paths: &Installation) -> Result<()> {
    state::ensure_layout(paths)?;
    recover_orphaned_document_mounts(paths)?;

    let records = state::read_sandbox_ownership_records(paths)?;
    let mounts = mount_infos()?;
    let roots = sandbox_roots(&paths.chroots(), &records, &mounts);
    let processes = SandboxProcessSnapshot::capture()?;
    let active = active_sandbox_roots(&paths.chroots(), &records, &mounts, &roots, &processes)?;
    let stale = roots.difference(&active).cloned().collect::<BTreeSet<_>>();

    for root in order_sandbox_roots_for_recovery(&paths.chroots(), stale, &records, &mounts) {
        // A launcher publishes ownership before mounting. Re-read all three
        // kernel/state views immediately before teardown so a concurrent live
        // primary or nested instance is never recovered as stale.
        let current_records = state::read_sandbox_ownership_records(paths)?;
        let current_mounts = mount_infos()?;
        let current_roots = sandbox_roots(&paths.chroots(), &current_records, &current_mounts);
        let current_processes = SandboxProcessSnapshot::capture()?;
        let current_active = active_sandbox_roots(
            &paths.chroots(),
            &current_records,
            &current_mounts,
            &current_roots,
            &current_processes,
        )?;
        if current_active.contains(&root) {
            continue;
        }

        terminate_chroot_processes(&root)?;
        unmount_under(&root)?;
        if mount_points_under(&root)?.is_empty() {
            remove_instance_root(&root)?;
            for record in &current_records {
                if record
                    .get("root")
                    .is_some_and(|value| Path::new(value) == root.as_path())
                {
                    if let Some(record_path) = record.get("_path") {
                        state::remove_run_record(Path::new(record_path))?;
                    }
                }
            }
        }
    }

    recover_orphaned_document_mounts(paths)
}

pub fn app_has_mounts(paths: &Installation, app_id: &str) -> Result<bool> {
    let root = paths.chroots().join(sandbox_name(app_id));
    Ok(!mount_points_under(&root)?.is_empty())
}

fn recover_orphaned_document_mounts(paths: &Installation) -> Result<()> {
    let chroots = paths.chroots();
    let chroots_identity = mount_identity(&chroots)?;
    for mountpoint in mount_points_under(&chroots)? {
        if !is_orphaned_regular_document_mount(&chroots, &mountpoint) {
            continue;
        }
        let unmount = |force| {
            let command = secure_mount::recover_orphaned_document_unmount_command(
                &chroots,
                chroots_identity,
                &mountpoint,
                force,
            )?;
            run_command(
                command,
                &format!("recover orphaned document mount {}", mountpoint.display()),
            )
        };
        if let Err(error) = unmount(false) {
            if !is_mounted(&mountpoint)? {
                continue;
            }
            eprintln!(
                "warning: orphaned document umount failed for {}: {error:#}",
                mountpoint.display()
            );
            unmount(true)?;
        }
    }
    Ok(())
}

fn is_orphaned_regular_document_mount(chroots: &Path, mountpoint: &Path) -> bool {
    let Ok(relative) = mountpoint.strip_prefix(chroots) else {
        return false;
    };
    let parts: Vec<_> = relative.components().collect();
    let [std::path::Component::Normal(app), std::path::Component::Normal(instance), std::path::Component::Normal(run), std::path::Component::Normal(user), std::path::Component::Normal(_uid), std::path::Component::Normal(doc), std::path::Component::Normal(_grant), std::path::Component::Normal(_file)] =
        parts.as_slice()
    else {
        return false;
    };
    *run == std::ffi::OsStr::new("run")
        && *user == std::ffi::OsStr::new("user")
        && *doc == std::ffi::OsStr::new("doc")
        && !chroots.join(app).join(instance).exists()
}

fn mount_identity(path: &Path) -> Result<(u64, u64)> {
    let metadata =
        fs::metadata(path).with_context(|| format!("read metadata for {}", path.display()))?;
    Ok((metadata.dev(), metadata.ino()))
}

fn sandbox_roots(
    chroots: &Path,
    records: &[BTreeMap<String, String>],
    mounts: &[MountInfo],
) -> BTreeSet<PathBuf> {
    let mut roots = records
        .iter()
        .filter_map(|record| record.get("root").map(PathBuf::from))
        .collect::<BTreeSet<_>>();
    for mount in mounts {
        if let Some(root) = chroot_root_for_mount(chroots, &mount.mountpoint) {
            roots.insert(root);
        }
    }
    roots
}

fn active_sandbox_roots(
    chroots: &Path,
    records: &[BTreeMap<String, String>],
    mounts: &[MountInfo],
    roots: &BTreeSet<PathBuf>,
    processes: &SandboxProcessSnapshot,
) -> Result<BTreeSet<PathBuf>> {
    let mut active = active_roots_from_records(records, processes)?;
    active.extend(
        roots
            .iter()
            .filter(|root| processes.references_root(root))
            .cloned(),
    );

    loop {
        let mut changed = false;
        for record in records {
            let Some(root) = record.get("root").map(PathBuf::from) else {
                continue;
            };
            if active.contains(&root) {
                if let Some(parent) = record.get("parent_root").map(PathBuf::from) {
                    changed |= active.insert(parent);
                }
            }
        }
        for mount in mounts {
            let Some(target_root) = chroot_root_for_mount(chroots, &mount.mountpoint) else {
                continue;
            };
            if !active.contains(&target_root) {
                continue;
            }
            if let Some(source_root) = chroot_root_for_mount(chroots, &mount.source) {
                changed |= active.insert(source_root);
            }
        }
        if !changed {
            break;
        }
    }
    Ok(active)
}

#[cfg(test)]
fn active_run_roots(paths: &Installation) -> Result<Vec<PathBuf>> {
    let records = state::read_sandbox_ownership_records(paths)?;
    let mounts = mount_infos()?;
    let roots = sandbox_roots(&paths.chroots(), &records, &mounts);
    let processes = SandboxProcessSnapshot::capture()?;
    Ok(
        active_sandbox_roots(&paths.chroots(), &records, &mounts, &roots, &processes)?
            .into_iter()
            .collect(),
    )
}

fn active_roots_from_records(
    records: &[BTreeMap<String, String>],
    processes: &SandboxProcessSnapshot,
) -> Result<BTreeSet<PathBuf>> {
    let mut active = BTreeSet::new();
    for record in records {
        let Some(root) = record.get("root").map(PathBuf::from) else {
            continue;
        };
        if processes.references_root(&root) || state::run_record_launcher_active(record)? {
            active.insert(root);
        }
    }

    // Nested Spawn roots are siblings of their parent root. Preserve the
    // ownership chain when a nested process survives so parent mounts used as
    // its nullfs sources cannot be recovered from underneath it.
    loop {
        let mut changed = false;
        for record in records {
            let Some(root) = record.get("root").map(PathBuf::from) else {
                continue;
            };
            if active.contains(&root) {
                if let Some(parent) = record.get("parent_root").map(PathBuf::from) {
                    changed |= active.insert(parent);
                }
            }
        }
        if !changed {
            break;
        }
    }
    Ok(active)
}

fn order_sandbox_roots_for_recovery(
    chroots: &Path,
    roots: BTreeSet<PathBuf>,
    records: &[BTreeMap<String, String>],
    mounts: &[MountInfo],
) -> Vec<PathBuf> {
    let mut dependencies = BTreeSet::new();
    for record in records {
        let Some(root) = record.get("root").map(PathBuf::from) else {
            continue;
        };
        if let Some(parent) = record.get("parent_root").map(PathBuf::from) {
            if roots.contains(&root) && roots.contains(&parent) {
                dependencies.insert((root, parent));
            }
        }
    }
    for mount in mounts {
        let Some(target_root) = chroot_root_for_mount(chroots, &mount.mountpoint) else {
            continue;
        };
        let Some(source_root) = chroot_root_for_mount(chroots, &mount.source) else {
            continue;
        };
        if target_root != source_root
            && roots.contains(&target_root)
            && roots.contains(&source_root)
        {
            dependencies.insert((target_root, source_root));
        }
    }
    topological_path_order(roots, &dependencies)
}

fn order_mounts_for_recovery(mounts: Vec<MountInfo>) -> Vec<MountInfo> {
    let paths = mounts
        .iter()
        .map(|mount| mount.mountpoint.clone())
        .collect::<BTreeSet<_>>();
    let mut dependencies = BTreeSet::new();
    for mount in &mounts {
        for possible_parent in &mounts {
            if mount.mountpoint != possible_parent.mountpoint
                && (mount.mountpoint.starts_with(&possible_parent.mountpoint)
                    || mount.source.starts_with(&possible_parent.mountpoint))
            {
                dependencies.insert((mount.mountpoint.clone(), possible_parent.mountpoint.clone()));
            }
        }
    }
    let ordered = topological_path_order(paths, &dependencies);
    let by_path = mounts
        .into_iter()
        .map(|mount| (mount.mountpoint.clone(), mount))
        .collect::<BTreeMap<_, _>>();
    ordered
        .into_iter()
        .filter_map(|path| by_path.get(&path).cloned())
        .collect()
}

fn topological_path_order(
    mut remaining: BTreeSet<PathBuf>,
    dependencies: &BTreeSet<(PathBuf, PathBuf)>,
) -> Vec<PathBuf> {
    let mut ordered = Vec::with_capacity(remaining.len());
    while !remaining.is_empty() {
        let ready = remaining
            .iter()
            .filter(|candidate| {
                !dependencies
                    .iter()
                    .any(|(before, after)| after == *candidate && remaining.contains(before))
            })
            .max_by(|left, right| {
                left.components()
                    .count()
                    .cmp(&right.components().count())
                    .then_with(|| left.cmp(right))
            })
            .cloned()
            .or_else(|| remaining.iter().next_back().cloned())
            .expect("remaining recovery path");
        remaining.remove(&ready);
        ordered.push(ready);
    }
    ordered
}

#[cfg(test)]
fn order_run_records_for_recovery(
    mut records: Vec<BTreeMap<String, String>>,
) -> Vec<BTreeMap<String, String>> {
    let parents = records
        .iter()
        .filter_map(|record| {
            record.get("root").map(|root| {
                (
                    PathBuf::from(root),
                    record.get("parent_root").map(PathBuf::from),
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
    records.sort_by(|left, right| {
        record_ownership_depth(right, &parents)
            .cmp(&record_ownership_depth(left, &parents))
            .then_with(|| right.get("root").cmp(&left.get("root")))
    });
    records
}

#[cfg(test)]
fn record_ownership_depth(
    record: &BTreeMap<String, String>,
    parents: &BTreeMap<PathBuf, Option<PathBuf>>,
) -> usize {
    let mut depth = 0;
    let mut seen = BTreeSet::new();
    let mut parent = record.get("parent_root").map(PathBuf::from);
    while let Some(root) = parent {
        if !seen.insert(root.clone()) {
            break;
        }
        depth += 1;
        parent = parents.get(&root).and_then(Clone::clone);
    }
    depth
}

#[cfg(test)]
pub(super) fn belongs_to_any_root(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path.starts_with(root))
}

pub(super) fn ensure_mountpoint_free(target: &Path) -> Result<()> {
    if is_mounted(target)? {
        bail!(
            "sandbox mountpoint is already mounted; clean it first: {}",
            target.display()
        );
    }
    Ok(())
}

fn is_mounted(target: &Path) -> Result<bool> {
    Ok(mount_points()?
        .iter()
        .any(|mountpoint| mountpoint == target))
}

fn mount_points_under(root: &Path) -> Result<Vec<PathBuf>> {
    let mut mounts: Vec<PathBuf> = mount_points()?
        .into_iter()
        .filter(|mountpoint| mountpoint.starts_with(root))
        .collect();
    sort_mountpoints_deepest_first(&mut mounts);
    Ok(mounts)
}

fn sort_mountpoints_deepest_first(mounts: &mut [PathBuf]) {
    mounts.sort_by(|left, right| {
        right
            .components()
            .count()
            .cmp(&left.components().count())
            .then_with(|| right.cmp(left))
    });
}

fn mount_points() -> Result<Vec<PathBuf>> {
    Ok(mount_infos()?
        .into_iter()
        .map(|mount| mount.mountpoint)
        .collect())
}

#[derive(Clone, Debug)]
struct MountInfo {
    source: PathBuf,
    mountpoint: PathBuf,
    options: String,
}

fn mount_infos() -> Result<Vec<MountInfo>> {
    let output = Command::new("mount").output().context("read mount table")?;
    if !output.status.success() {
        bail!("mount command failed with status {}", output.status);
    }
    let mount_table = String::from_utf8(output.stdout)?;
    Ok(mount_table
        .lines()
        .filter_map(|line| {
            line.split_once(" on ")
                .and_then(|(source, rest)| {
                    rest.split_once(" (")
                        .map(|(mountpoint, options)| (source, mountpoint, options))
                })
                .map(|(source, mountpoint, options)| MountInfo {
                    source: PathBuf::from(source),
                    mountpoint: PathBuf::from(mountpoint),
                    options: options.trim_end_matches(')').to_string(),
                })
        })
        .collect())
}

fn unmount_under(root: &Path) -> Result<()> {
    let mounts = mount_infos()?
        .into_iter()
        .filter(|mount| mount.mountpoint.starts_with(root))
        .collect::<Vec<_>>();
    let ordered = order_mounts_for_recovery(mounts)
        .into_iter()
        .map(|mount| mount.mountpoint)
        .collect();
    unmount_mountpoints(ordered)
}

pub(super) fn terminate_chroot_processes(root: &Path) -> Result<()> {
    let initial = chroot_processes(root)?;
    if initial.is_empty() {
        return Ok(());
    }
    eprintln!(
        "terminating {} remaining sandbox process(es)",
        initial.len()
    );

    let remaining = terminate_processes_with(
        || chroot_processes(root),
        |pid, signal| unsafe {
            libc::kill(pid, signal);
        },
        || thread::sleep(Duration::from_millis(100)),
    )?;
    if !remaining.is_empty() {
        bail!("sandbox processes survived SIGKILL: {remaining:?}");
    }
    Ok(())
}

fn chroot_processes(root: &Path) -> Result<Vec<i32>> {
    Ok(SandboxProcessSnapshot::capture()?
        .pids_referencing_root(root)
        .into_iter()
        .filter(|pid| *pid != std::process::id() as i32)
        .collect())
}

fn terminate_processes_with(
    mut processes: impl FnMut() -> Result<Vec<i32>>,
    mut signal: impl FnMut(i32, i32),
    mut pause: impl FnMut(),
) -> Result<Vec<i32>> {
    for requested_signal in [libc::SIGTERM, libc::SIGKILL] {
        for _ in 0..20 {
            let pids = processes()?;
            if pids.is_empty() {
                return Ok(pids);
            }
            for pid in pids {
                signal(pid, requested_signal);
            }
            pause();
        }
    }
    processes()
}

pub(super) fn chroot_root_for_mount(chroot_root: &Path, mountpoint: &Path) -> Option<PathBuf> {
    let mut candidate = mountpoint.parent();
    while let Some(path) = candidate {
        if path == chroot_root {
            break;
        }
        if path.starts_with(chroot_root) && path.join(".flatpak-info").is_file() {
            return Some(path.to_path_buf());
        }
        candidate = path.parent();
    }
    None
}

pub(super) fn remove_instance_root(root: &Path) -> Result<()> {
    match fs::remove_dir_all(root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove sandbox root {}", root.display())),
    }
}

#[cfg(test)]
#[path = "tests/stale_sandbox_recovery.rs"]
mod tests;

fn unmount_mountpoints(mountpoints: Vec<PathBuf>) -> Result<()> {
    let mut errors = Vec::new();
    for mountpoint in mountpoints {
        let read_only = mountpoint_is_read_only(&mountpoint).unwrap_or(false);
        if let Err(error) = unmount_mountpoint(&mountpoint, read_only, "recover umount") {
            errors.push(format!("{error:#}"));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        bail!("stale mount recovery failed:\n{}", errors.join("\n"));
    }
}

fn mountpoint_is_read_only(mountpoint: &Path) -> Result<bool> {
    Ok(mount_infos()?
        .into_iter()
        .find(|mount| mount.mountpoint == mountpoint)
        .map(|mount| {
            mount
                .options
                .split(',')
                .map(str::trim)
                .any(|option| option == "read-only")
        })
        .unwrap_or(false))
}

pub(crate) fn unmount_mountpoint(mountpoint: &Path, allow_force: bool, action: &str) -> Result<()> {
    let (root, target_relative, root_identity) = sandbox_mount_target(mountpoint)?;
    unmount_mountpoint_with(
        mountpoint,
        allow_force,
        action,
        is_mounted,
        |path, force| {
            if path != mountpoint {
                bail!("secure unmount target changed during retry");
            }
            let command =
                secure_mount::unmount_command(&root, root_identity, &target_relative, force)?;
            let force_label = if force { " -f" } else { "" };
            run_command(
                command,
                &format!("{action}{force_label} {}", path.display()),
            )
        },
        || thread::sleep(Duration::from_millis(250)),
    )
}

fn sandbox_mount_target(mountpoint: &Path) -> Result<(PathBuf, PathBuf, (u64, u64))> {
    let root = mountpoint
        .ancestors()
        .find(|ancestor| ancestor.join(".flatpak-info").is_file())
        .context("mountpoint is not below a freebsd-flatpak sandbox root")?
        .to_path_buf();
    let target_relative = mountpoint
        .strip_prefix(&root)
        .context("mountpoint is outside its sandbox root")?
        .to_path_buf();
    if target_relative.as_os_str().is_empty() {
        bail!("refusing to unmount sandbox root");
    }
    let metadata =
        fs::metadata(&root).with_context(|| format!("inspect sandbox root {}", root.display()))?;
    Ok((root, target_relative, (metadata.dev(), metadata.ino())))
}

pub(super) fn unmount_mountpoint_with(
    mountpoint: &Path,
    allow_force: bool,
    _action: &str,
    mut mounted: impl FnMut(&Path) -> Result<bool>,
    mut unmount: impl FnMut(&Path, bool) -> Result<()>,
    _retry_pause: impl FnMut(),
) -> Result<()> {
    if !mounted(mountpoint)? {
        return Ok(());
    }
    let normal_error = match unmount(mountpoint, false) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    if !mounted(mountpoint)? {
        return Ok(());
    }

    if allow_force && mounted(mountpoint)? {
        eprintln!(
            "warning: normal unmount stayed busy for read-only mount {}; trying umount -f",
            mountpoint.display()
        );
        unmount(mountpoint, true)?;
        return Ok(());
    }

    if !mounted(mountpoint)? {
        return Ok(());
    }

    Err(normal_error)
}

pub(super) fn run_command(mut command: Command, action: &str) -> Result<()> {
    let status = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| action.to_string())?;
    if !status.success() {
        bail!("{action} failed with status {status}");
    }
    Ok(())
}
