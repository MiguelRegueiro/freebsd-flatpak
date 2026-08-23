use super::launch_application::FlatpakApp;
use crate::flatpak_metadata::value;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) fn launch_args(app: &FlatpakApp, translated_args: Vec<String>) -> Result<Vec<String>> {
    let mut args = compatibility_args(app, &translated_args)?;
    args.extend(translated_args);
    Ok(args)
}

pub(super) fn compatibility_args(
    app: &FlatpakApp,
    translated_args: &[String],
) -> Result<Vec<String>> {
    if has_ozone_platform_arg(translated_args) {
        return Ok(Vec::new());
    }

    let metadata_path = app.app_dir.join("metadata");
    let metadata = fs::read_to_string(&metadata_path)
        .with_context(|| format!("read Flatpak metadata {}", metadata_path.display()))?;

    if app_uses_electron_base(&metadata) && app_requests_socket(&metadata, "wayland") {
        return Ok(vec!["--ozone-platform=wayland".to_string()]);
    }

    Ok(Vec::new())
}

fn app_uses_electron_base(metadata: &str) -> bool {
    value(metadata, "Application", "base")
        .is_some_and(|base| base.starts_with("app/org.electronjs.Electron2.BaseApp/"))
}

fn app_requests_socket(metadata: &str, socket: &str) -> bool {
    value(metadata, "Context", "sockets")
        .map(|sockets| sockets.split(';').any(|entry| entry == socket))
        .unwrap_or(false)
}

fn has_ozone_platform_arg(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == "--ozone-platform" || arg.starts_with("--ozone-platform="))
}

#[derive(Debug, Clone)]
pub(super) struct EntryLaunch {
    chroot_path: String,
    mode: EntryLaunchMode,
}

#[derive(Debug, Clone)]
enum EntryLaunchMode {
    LinuxElf,
    Direct,
}

impl EntryLaunch {
    pub(super) fn display(&self, args: &[String]) -> String {
        let mut display = match self.mode {
            EntryLaunchMode::LinuxElf => {
                format!("/lib64/ld-linux-x86-64.so.2 {}", self.chroot_path)
            }
            EntryLaunchMode::Direct => self.chroot_path.clone(),
        };
        for arg in args {
            display.push(' ');
            display.push_str(arg);
        }
        display
    }

    pub(super) fn append_command_args(&self, command: &mut Command, args: &[String]) {
        match self.mode {
            EntryLaunchMode::LinuxElf => {
                command
                    .arg("/lib64/ld-linux-x86-64.so.2")
                    .arg(&self.chroot_path);
            }
            EntryLaunchMode::Direct => {
                command.arg(&self.chroot_path);
            }
        }
        command.args(args);
    }
}

pub(super) fn resolve_entry(app: &FlatpakApp) -> Result<EntryLaunch> {
    let app_files = app.app_dir.join("files");
    let chroot_path = chroot_entry_path(&app.command);
    let host_entry = host_app_path(&app_files, &chroot_path)?;
    if fs::symlink_metadata(&host_entry).is_err() {
        bail!("entry executable does not exist: {}", host_entry.display());
    }
    let probe_entry =
        resolve_app_symlink_for_probe(&app_files, &host_entry).unwrap_or(host_entry.clone());
    let mode = if is_linux_elf(&probe_entry) {
        EntryLaunchMode::LinuxElf
    } else {
        EntryLaunchMode::Direct
    };
    Ok(EntryLaunch { chroot_path, mode })
}

fn host_app_path(app_files: &Path, chroot_path: &str) -> Result<PathBuf> {
    if let Some(relative) = chroot_path.strip_prefix("/app/") {
        return Ok(app_files.join(relative));
    }
    if chroot_path.starts_with('/') {
        bail!("entry path must be inside /app for this POC: {chroot_path}");
    }
    Ok(app_files.join("bin").join(chroot_path))
}

fn resolve_app_symlink_for_probe(app_files: &Path, path: &Path) -> Result<PathBuf> {
    let mut current = path.to_path_buf();
    for _ in 0..8 {
        let metadata = fs::symlink_metadata(&current)
            .with_context(|| format!("read metadata for {}", current.display()))?;
        if !metadata.file_type().is_symlink() {
            return Ok(current);
        }

        let target = fs::read_link(&current)
            .with_context(|| format!("read symlink {}", current.display()))?;
        current = if target.is_absolute() {
            let target = target
                .to_str()
                .context("absolute symlink target is not UTF-8")?;
            host_app_path(app_files, target)?
        } else {
            current
                .parent()
                .context("symlink has no parent")?
                .join(target)
        };
    }

    bail!("too many symlink hops resolving {}", path.display());
}

fn chroot_entry_path(command: &str) -> String {
    if command.starts_with('/') {
        command.to_string()
    } else {
        format!("/app/bin/{command}")
    }
}

pub(super) fn sandbox_name(app_id: &str) -> String {
    app_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub(super) fn numeric_id(program: &str, arg: &str) -> Result<u32> {
    let output = Command::new(program)
        .arg(arg)
        .output()
        .with_context(|| format!("run {program} {arg}"))?;
    if !output.status.success() {
        bail!("{program} {arg} failed with status {}", output.status);
    }
    let text = String::from_utf8(output.stdout)?.trim().to_string();
    text.parse::<u32>()
        .with_context(|| format!("parse numeric id from {text:?}"))
}

pub(super) fn numeric_ids(program: &str, arg: &str) -> Result<Vec<u32>> {
    let output = Command::new(program)
        .arg(arg)
        .output()
        .with_context(|| format!("run {program} {arg}"))?;
    if !output.status.success() {
        bail!("{program} {arg} failed with status {}", output.status);
    }
    let text = String::from_utf8(output.stdout)?;
    text.split_whitespace()
        .map(|value| {
            value
                .parse::<u32>()
                .with_context(|| format!("parse numeric id from {value:?}"))
        })
        .collect()
}

pub(super) fn join_numeric_ids(ids: &[u32]) -> String {
    ids.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
}

pub(super) fn host_user(uid: u32) -> String {
    std::env::var("USER").unwrap_or_else(|_| uid.to_string())
}

fn is_linux_elf(path: &Path) -> bool {
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    bytes.len() >= 20 && &bytes[0..4] == b"\x7fELF" && bytes.get(7).copied() == Some(0)
}

#[cfg(test)]
#[path = "tests/application_entrypoint.rs"]
mod tests;
