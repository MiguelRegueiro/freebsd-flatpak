use crate::host_resources::audio;
use crate::installation::{self as state, installation_paths::Installation};
use crate::sandbox::filesystem_grants as filesystem;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;

pub(crate) fn cmd_permissions(paths: &Installation, args: Vec<String>) -> Result<()> {
    if args.len() != 1 {
        bail!("usage: flatpak permissions <app-id>");
    }
    let record = state::get_app(paths, &args[0])?;
    let user = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
    let sandbox_root = paths.chroots().join(&record.app_id);
    let uid = numeric_id("id", "-u")?;
    let xdg_runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{uid}")));
    let metadata_path = state::absolute(paths, &record.app_dir).join("metadata");
    let host_filesystem = filesystem::HostFilesystem::from_metadata_file_for_user(
        &metadata_path,
        &user,
        paths.data_root(),
        &sandbox_root,
    )?;
    let host_audio = audio::HostAudio::from_metadata_file(&metadata_path, &xdg_runtime_dir, uid)?;

    println!("Filesystem permissions for {}", record.app_id);
    println!("Metadata filesystems:");
    if host_filesystem.permissions().is_empty() {
        println!("  <none>");
    } else {
        for permission in host_filesystem.permissions() {
            let create = if permission.create() { ", create" } else { "" };
            println!(
                "  {:<28} {}{}",
                permission.original(),
                permission.access().label(),
                create
            );
        }
    }

    println!("Resolved nullfs grants:");
    if host_filesystem.grants().is_empty() {
        println!("  <none>");
    } else {
        for grant in host_filesystem.grants() {
            println!(
                "  {:<42} -> {:<42} {} ({})",
                grant.host_path().display(),
                grant.sandbox_path().display(),
                grant.access().label(),
                grant.source_permission()
            );
        }
    }

    if !host_filesystem.warnings().is_empty() {
        println!("Warnings:");
        for warning in host_filesystem.warnings() {
            println!("  {warning}");
        }
    }

    println!("Socket permissions:");
    if host_audio.sockets().is_empty() {
        println!("  <none>");
    } else {
        for socket in host_audio.sockets() {
            println!("  {socket}");
        }
    }

    println!("Resolved audio bridge:");
    let audio_lines = host_audio.describe();
    if audio_lines.is_empty() {
        println!("  <none>");
    } else {
        for line in audio_lines {
            println!("  {line}");
        }
    }
    if !host_audio.warnings().is_empty() {
        println!("Audio warnings:");
        for warning in host_audio.warnings() {
            println!("  {warning}");
        }
    }

    Ok(())
}
fn numeric_id(program: &str, arg: &str) -> Result<u32> {
    let output = std::process::Command::new(program)
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
