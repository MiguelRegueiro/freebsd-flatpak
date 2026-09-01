use super::launch_application::FlatpakApp;
use super::sandbox_identity::SandboxIdentity;
use crate::flatpak_metadata::value;
use crate::installation::ExtensionMountPlan;
use anyhow::{bail, Context, Result};
use std::fs;
use std::os::unix::fs as unix_fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub(super) fn prepare_root(
    root: &Path,
    identity: &SandboxIdentity,
    runtime_etc: &Path,
    network_enabled: bool,
) -> Result<()> {
    for dir in [
        "app",
        "usr",
        "dev",
        "proc",
        "sys",
        "tmp",
        "var/data",
        "var/cache",
        "var/config",
    ] {
        fs::create_dir_all(root.join(dir))
            .with_context(|| format!("create {}", root.join(dir).display()))?;
    }
    fs::create_dir_all(
        root.join("run")
            .join("user")
            .join(identity.uid().to_string()),
    )
    .with_context(|| format!("create {}", root.join("run/user").display()))?;

    make_link("usr/bin", &root.join("bin"))?;
    make_link("usr/lib", &root.join("lib"))?;
    make_link("usr/lib64", &root.join("lib64"))?;
    prepare_etc_overlay(root, runtime_etc, network_enabled)?;
    prepare_identity_files(&root.join("etc"), identity)?;
    Ok(())
}

pub(super) fn prepare_etc_overlay(
    root: &Path,
    runtime_etc: &Path,
    network_enabled: bool,
) -> Result<()> {
    let etc = root.join("etc");
    if let Ok(metadata) = fs::symlink_metadata(&etc) {
        if metadata.file_type().is_symlink() {
            fs::remove_file(&etc).with_context(|| format!("replace {}", etc.display()))?;
        } else if !metadata.is_dir() {
            bail!("{} exists and is not a directory", etc.display());
        }
    }
    fs::create_dir_all(&etc).with_context(|| format!("create {}", etc.display()))?;

    for entry in
        fs::read_dir(runtime_etc).with_context(|| format!("read {}", runtime_etc.display()))?
    {
        let entry = entry.with_context(|| format!("read entry in {}", runtime_etc.display()))?;
        let name = entry.file_name();
        let Some(name_text) = name.to_str() else {
            continue;
        };
        if matches!(
            name_text,
            "passwd" | "group" | "resolv.conf" | "hosts" | "os-release"
        ) {
            continue;
        }
        let link = etc.join(name_text);
        if fs::symlink_metadata(&link).is_ok() {
            continue;
        }
        unix_fs::symlink(format!("/usr/etc/{name_text}"), &link)
            .with_context(|| format!("symlink {} -> /usr/etc/{name_text}", link.display()))?;
    }

    prepare_runtime_os_release(&etc, runtime_etc)?;

    prepare_host_resolver_file(
        &etc,
        "resolv.conf",
        Path::new("/etc/resolv.conf"),
        network_enabled,
    )?;
    prepare_host_resolver_file(&etc, "hosts", Path::new("/etc/hosts"), network_enabled)?;
    Ok(())
}

fn prepare_runtime_os_release(etc: &Path, runtime_etc: &Path) -> Result<()> {
    let target = etc.join("os-release");
    remove_regular_overlay_file(&target)?;

    let Some(runtime_files) = runtime_etc.parent() else {
        return Ok(());
    };
    if !runtime_files.join("lib/os-release").is_file() {
        return Ok(());
    }

    unix_fs::symlink("../usr/lib/os-release", &target)
        .with_context(|| format!("symlink {} -> ../usr/lib/os-release", target.display()))
}

fn prepare_host_resolver_file(
    etc: &Path,
    name: &str,
    host_path: &Path,
    network_enabled: bool,
) -> Result<()> {
    let target = etc.join(name);
    if !network_enabled {
        remove_regular_overlay_file(&target)?;
        return Ok(());
    }
    if !host_path.exists() {
        remove_regular_overlay_file(&target)?;
        return Ok(());
    }
    if let Ok(metadata) = fs::symlink_metadata(&target) {
        if metadata.file_type().is_symlink() || metadata.is_file() {
            fs::remove_file(&target).with_context(|| format!("replace {}", target.display()))?;
        } else {
            bail!("{} exists and is not a file", target.display());
        }
    }
    let data = fs::read(host_path).with_context(|| format!("read {}", host_path.display()))?;
    fs::write(&target, data).with_context(|| format!("write {}", target.display()))?;
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644))
        .with_context(|| format!("set permissions on {}", target.display()))?;
    Ok(())
}

fn prepare_identity_files(etc: &Path, identity: &SandboxIdentity) -> Result<()> {
    let files = [
        ("passwd", identity.passwd_contents()),
        ("group", identity.group_contents()),
    ];
    for (name, contents) in files {
        let target = etc.join(name);
        remove_regular_overlay_file(&target)?;
        fs::write(&target, contents).with_context(|| format!("write {}", target.display()))?;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644))
            .with_context(|| format!("set permissions on {}", target.display()))?;
    }
    Ok(())
}

fn remove_regular_overlay_file(path: &Path) -> Result<()> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if metadata.file_type().is_symlink() || metadata.is_file() {
        fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}

pub(super) fn app_allows_network(metadata: &str) -> bool {
    value(metadata, "Context", "shared")
        .map(|shared| {
            shared
                .split(';')
                .map(str::trim)
                .any(|permission| permission == "network")
        })
        .unwrap_or(false)
}

pub(super) fn write_flatpak_info(
    root: &Path,
    app: &FlatpakApp,
    instance_id: &str,
    extensions: &ExtensionMountPlan,
) -> Result<()> {
    let list = |items: Vec<String>| {
        if items.is_empty() {
            String::new()
        } else {
            format!("{};", items.join(";"))
        }
    };
    let data = format!(
        "\
[Application]
name={}
runtime={}

[Instance]
instance-id={}
flatpak-version=1.12.0
app-extensions={}
runtime-extensions={}

[Context]
filesystems=
",
        app.app_id,
        app.runtime_ref,
        instance_id,
        list(extensions.app_info()),
        list(extensions.runtime_info())
    );
    fs::write(root.join(".flatpak-info"), data)
        .with_context(|| format!("write {}", root.join(".flatpak-info").display()))
}

fn make_link(target: &str, link: &Path) -> Result<()> {
    if fs::symlink_metadata(link).is_ok() {
        return Ok(());
    }
    unix_fs::symlink(target, link)
        .with_context(|| format!("symlink {} -> {target}", link.display()))
}

#[cfg(test)]
#[path = "tests/sandbox_root.rs"]
mod tests;
