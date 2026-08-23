use crate::installation::installation_paths::Installation;
use anyhow::{Context, Result};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::process::{Command, Stdio};

pub fn refresh_export_caches(paths: &Installation) -> Result<()> {
    let applications = paths.data_home().join("applications");
    if applications.is_dir() {
        run_optional_cache_command(
            "update-desktop-database",
            &[applications.as_path()],
            "refresh desktop MIME cache",
        )?;
    }

    let hicolor = paths.data_home().join("icons").join("hicolor");
    if hicolor.is_dir() {
        ensure_hicolor_index(&hicolor)?;
        run_optional_cache_command(
            "gtk-update-icon-cache",
            &[
                Path::new("-q"),
                Path::new("-t"),
                Path::new("-f"),
                hicolor.as_path(),
            ],
            "refresh icon cache",
        )?;
    }

    Ok(())
}

fn ensure_hicolor_index(hicolor: &Path) -> Result<()> {
    let index = hicolor.join("index.theme");
    if index.exists() {
        return Ok(());
    }
    let data = "\
[Icon Theme]
Name=Hicolor
Comment=Fallback icon theme for FreeBSD Flatpak exports
Directories=scalable/apps,symbolic/apps

[scalable/apps]
Size=128
Type=Scalable
Context=Applications

[symbolic/apps]
Size=16
Type=Scalable
Context=Applications
";
    fs::write(&index, data).with_context(|| format!("write {}", index.display()))
}

fn run_optional_cache_command(program: &str, args: &[&Path], action: &str) -> Result<()> {
    let mut command = Command::new(program);
    for arg in args {
        command.arg(arg);
    }
    let status = command
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();

    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => {
            eprintln!("warning: {action} failed with status {status}");
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| action.to_string()),
    }
}
