use super::launch_application::FlatpakApp;
use crate::flatpak_compatibility::FLATPAK_COMPATIBILITY_VERSION;
use crate::flatpak_metadata::value;
use crate::installation::{deployment_marker, AppRecord};
use anyhow::{bail, Context, Result};
use glib::KeyFile;
use std::fs;
use std::os::unix::fs as unix_fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub(super) fn prepare_root(
    root: &Path,
    uid: u32,
    gid: u32,
    user: &str,
    home: &Path,
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
    let user_runtime = root.join("run").join("user").join(uid.to_string());
    fs::create_dir_all(&user_runtime)
        .with_context(|| format!("create {}", root.join("run/user").display()))?;
    make_link("../../../.flatpak-info", &user_runtime.join("flatpak-info"))?;

    make_link("usr/bin", &root.join("bin"))?;
    make_link("usr/lib", &root.join("lib"))?;
    make_link("usr/lib64", &root.join("lib64"))?;
    prepare_etc_overlay(root, runtime_etc, network_enabled)?;
    write_identity_files(root, uid, gid, user, home)?;
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
        if matches!(name_text, "passwd" | "group" | "resolv.conf" | "hosts") {
            continue;
        }
        let link = etc.join(name_text);
        if fs::symlink_metadata(&link).is_ok() {
            continue;
        }
        unix_fs::symlink(format!("/usr/etc/{name_text}"), &link)
            .with_context(|| format!("symlink {} -> /usr/etc/{name_text}", link.display()))?;
    }

    prepare_host_resolver_file(
        &etc,
        "resolv.conf",
        Path::new("/etc/resolv.conf"),
        network_enabled,
    )?;
    prepare_host_resolver_file(&etc, "hosts", Path::new("/etc/hosts"), network_enabled)?;
    Ok(())
}

fn write_identity_files(root: &Path, uid: u32, gid: u32, user: &str, home: &Path) -> Result<()> {
    let home = path_value(home)?;
    validate_identity_field("user name", user)?;
    validate_identity_field("home directory", &home)?;
    let passwd = format!(
        "{user}:x:{uid}:{gid}:{user}:{home}:/bin/sh\nnfsnobody:x:65534:65534:Unmapped user:/:/sbin/nologin\n"
    );
    let group = format!("{user}:x:{gid}:{user}\nnfsnobody:x:65534:\n");
    fs::write(root.join("etc/passwd"), passwd)
        .with_context(|| format!("write {}", root.join("etc/passwd").display()))?;
    fs::write(root.join("etc/group"), group)
        .with_context(|| format!("write {}", root.join("etc/group").display()))?;
    Ok(())
}

fn validate_identity_field(label: &str, value: &str) -> Result<()> {
    if value.contains([':', '\n', '\r']) {
        bail!("invalid sandbox {label} {value:?}");
    }
    Ok(())
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

fn remove_regular_overlay_file(path: &Path) -> Result<()> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if metadata.file_type().is_symlink() || metadata.is_file() {
        fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}

pub(super) fn app_allows_network(metadata_path: &Path) -> Result<bool> {
    let metadata = fs::read_to_string(metadata_path)
        .with_context(|| format!("read {}", metadata_path.display()))?;
    Ok(value(&metadata, "Context", "shared")
        .map(|shared| {
            shared
                .split(';')
                .map(str::trim)
                .any(|permission| permission == "network")
        })
        .unwrap_or(false))
}

pub(super) fn write_flatpak_info(
    root: &Path,
    app: &FlatpakApp,
    deployment: &AppRecord,
    instance_id: &str,
    instance_path: &Path,
    app_extensions: &[FlatpakInfoExtension<'_>],
    runtime_extensions: &[FlatpakInfoExtension<'_>],
) -> Result<()> {
    let keyfile = KeyFile::new();
    keyfile.set_string("Application", "name", &app.app_id);
    keyfile.set_string(
        "Application",
        "runtime",
        &fully_qualified_runtime_ref(&app.runtime_ref),
    );

    keyfile.set_string("Instance", "instance-id", instance_id);
    keyfile.set_string("Instance", "instance-path", &path_value(instance_path)?);
    keyfile.set_string(
        "Instance",
        "app-path",
        &path_value(&app.app_dir.join("files"))?,
    );
    keyfile.set_string("Instance", "app-commit", &deployment.app_commit);
    keyfile.set_string(
        "Instance",
        "runtime-path",
        &path_value(&app.runtime_dir.join("files"))?,
    );
    keyfile.set_string("Instance", "runtime-commit", &deployment.runtime_commit);
    set_extension_info(&keyfile, "app-extensions", app_extensions)?;
    set_extension_info(&keyfile, "runtime-extensions", runtime_extensions)?;
    keyfile.set_string("Instance", "branch", &deployment.branch);
    keyfile.set_string("Instance", "arch", &deployment.arch);
    keyfile.set_string("Instance", "flatpak-version", FLATPAK_COMPATIBILITY_VERSION);
    keyfile.set_boolean("Instance", "session-bus-proxy", true);
    keyfile.set_boolean("Instance", "system-bus-proxy", true);

    // Filesystem permissions are reported separately once their effective
    // Flatpak semantics are supported, rather than echoing unimplemented
    // permissions from the application metadata.
    keyfile.set_string("Context", "filesystems", "");

    let data = keyfile.to_data();
    fs::write(root.join(".flatpak-info"), data.as_bytes())
        .with_context(|| format!("write {}", root.join(".flatpak-info").display()))
}

fn fully_qualified_runtime_ref(runtime_ref: &str) -> String {
    if runtime_ref.starts_with("runtime/") {
        runtime_ref.to_string()
    } else {
        format!("runtime/{runtime_ref}")
    }
}

pub(super) struct FlatpakInfoExtension<'a> {
    pub ref_name: &'a str,
    pub checkout_dir: &'a Path,
}

fn set_extension_info(
    keyfile: &KeyFile,
    key: &str,
    extensions: &[FlatpakInfoExtension<'_>],
) -> Result<()> {
    let mut values = extensions
        .iter()
        .map(|extension| {
            let id = extension
                .ref_name
                .strip_prefix("runtime/")
                .and_then(|value| value.split('/').next())
                .with_context(|| format!("invalid extension ref {}", extension.ref_name))?;
            let (_, commit) = deployment_marker(extension.checkout_dir)?.with_context(|| {
                format!(
                    "extension checkout has no deployment marker: {}",
                    extension.checkout_dir.display()
                )
            })?;
            Ok(format!("{id}={commit}"))
        })
        .collect::<Result<Vec<_>>>()?;
    values.sort();
    values.dedup();
    if !values.is_empty() {
        keyfile.set_value("Instance", key, &format!("{};", values.join(";")));
    }
    Ok(())
}

fn path_value(path: &Path) -> Result<String> {
    path.to_str().map(str::to_string).with_context(|| {
        format!(
            "Flatpak metadata path is not valid UTF-8: {}",
            path.display()
        )
    })
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
