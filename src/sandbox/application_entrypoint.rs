use super::launch_application::FlatpakApp;
use crate::flatpak_metadata::value;
use anyhow::{bail, Context, Result};
use std::ffi::OsString;
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
}

impl EntryLaunch {
    pub(super) fn display(&self, args: &[String]) -> String {
        let mut display = self.chroot_path.clone();
        for arg in args {
            display.push(' ');
            display.push_str(arg);
        }
        display
    }

    #[cfg(test)]
    pub(super) fn append_command_args(&self, command: &mut Command, args: &[String]) {
        // Let execve(2) select an ELF executable's PT_INTERP after chroot(2).
        // Invoking ld-linux as the executable changes /proc/self/exe to the
        // loader. Frameworks such as Flutter use that link to find files next
        // to their application executable.
        command.arg(&self.chroot_path);
        command.args(args);
    }

    pub(super) fn command_args(&self, args: &[String]) -> Vec<OsString> {
        let mut command = Vec::with_capacity(args.len() + 1);
        command.push(OsString::from(&self.chroot_path));
        command.extend(args.iter().map(OsString::from));
        command
    }
}

pub(super) fn resolve_entry(app: &FlatpakApp) -> Result<EntryLaunch> {
    let app_files = app.app_dir.join("files");
    let chroot_path = chroot_entry_path(&app.command);
    let host_entry = host_app_path(&app_files, &chroot_path)?;
    if fs::symlink_metadata(&host_entry).is_err() {
        bail!("entry executable does not exist: {}", host_entry.display());
    }
    Ok(EntryLaunch { chroot_path })
}

fn host_app_path(app_files: &Path, chroot_path: &str) -> Result<PathBuf> {
    if let Some(relative) = chroot_path.strip_prefix("/app/") {
        return Ok(app_files.join(relative));
    }
    if chroot_path.starts_with('/') {
        bail!("entry path must be inside /app: {chroot_path}");
    }
    Ok(app_files.join("bin").join(chroot_path))
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

pub(super) fn host_user(uid: u32) -> String {
    std::env::var("USER").unwrap_or_else(|_| uid.to_string())
}

#[cfg(test)]
#[path = "tests/application_entrypoint.rs"]
mod tests;
