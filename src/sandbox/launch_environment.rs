use super::launch_application::FlatpakApp;
use crate::desktop_integration::DesktopSession;
use crate::flatpak_metadata::{section_entries, value};
use crate::installation as runtime;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn launch_env(
    app: &FlatpakApp,
    desktop: &DesktopSession,
    uid: u32,
    user: &str,
    host_data_home: &Path,
) -> Vec<(String, String)> {
    let mut env = vec![
        ("HOME".to_string(), "/var/data".to_string()),
        ("USER".to_string(), user.to_string()),
        ("LOGNAME".to_string(), user.to_string()),
        ("SHELL".to_string(), "/bin/sh".to_string()),
        ("container".to_string(), "flatpak".to_string()),
        ("FLATPAK_ID".to_string(), app.app_id.clone()),
        // Zypak's spawn strategy requires Linux PID namespaces across the nested jail;
        // its mimic strategy still uses the secure SANDBOX Spawn path for renderers.
        (
            "ZYPAK_ZYGOTE_STRATEGY_SPAWN".to_string(),
            "false".to_string(),
        ),
        ("XDG_RUNTIME_DIR".to_string(), format!("/run/user/{uid}")),
        ("WAYLAND_DISPLAY".to_string(), desktop.wayland_display.clone()),
        ("XDG_SESSION_TYPE".to_string(), "wayland".to_string()),
        ("LANG".to_string(), "C.UTF-8".to_string()),
        ("LC_ALL".to_string(), "C.UTF-8".to_string()),
        ("GDK_BACKEND".to_string(), "wayland".to_string()),
        ("GSK_RENDERER".to_string(), "cairo".to_string()),
        ("XDG_DATA_HOME".to_string(), "/var/data".to_string()),
        (
            "HOST_XDG_DATA_HOME".to_string(),
            host_data_home.display().to_string(),
        ),
        ("XDG_CONFIG_HOME".to_string(), "/var/config".to_string()),
        ("XDG_CACHE_HOME".to_string(), "/var/cache".to_string()),
        ("XDG_CONFIG_DIRS".to_string(), "/app/etc/xdg:/etc/xdg".to_string()),
        (
            "XDG_DATA_DIRS".to_string(),
            "/app/share:/usr/share:/usr/share/runtime/share:/run/host/share".to_string(),
        ),
        (
            "GI_TYPELIB_PATH".to_string(),
            "/app/lib/girepository-1.0:/usr/lib/x86_64-linux-gnu/girepository-1.0:/usr/lib/girepository-1.0"
                .to_string(),
        ),
        (
            "LD_LIBRARY_PATH".to_string(),
            "/app/lib:/app/lib64:/usr/lib/x86_64-linux-gnu:/usr/lib:/usr/lib64".to_string(),
        ),
        ("PATH".to_string(), "/app/bin:/usr/bin:/bin".to_string()),
    ];

    if let Some(display) = &desktop.display {
        env.push(("DISPLAY".to_string(), display.clone()));
    }
    push_host_env(&mut env, "XDG_CURRENT_DESKTOP");
    push_host_env(&mut env, "XDG_SESSION_DESKTOP");
    push_host_env(&mut env, "MOZ_ENABLE_WAYLAND");
    push_host_env(&mut env, "FREEBSD_FLATPAK_TRACE_LINUX_COMPAT");
    if let Some(address) = desktop.chroot_dbus_address(uid) {
        env.push(("DBUS_SESSION_BUS_ADDRESS".to_string(), address));
    }

    env
}

fn push_host_env(env: &mut Vec<(String, String)>, key: &str) {
    if let Ok(value) = std::env::var(key) {
        if !value.is_empty() {
            env.push((key.to_string(), value));
        }
    }
}

pub(super) fn metadata_env(
    runtime_metadata: &str,
    effective_app_metadata: &str,
    base_env: &[(String, String)],
) -> Vec<(String, String)> {
    let mut env = base_env.to_vec();
    let mut updates = Vec::new();
    for metadata in [runtime_metadata, effective_app_metadata] {
        for (key, raw_value) in section_entries(metadata, "Environment") {
            let value = expand_env_value(&raw_value, &env);
            merge_env(&mut env, vec![(key.clone(), value.clone())]);
            merge_env(&mut updates, vec![(key, value)]);
        }
    }
    updates
}

fn expand_env_value(value: &str, env: &[(String, String)]) -> String {
    let mut expanded = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '$' {
            expanded.push(ch);
            continue;
        }

        if chars.peek() == Some(&'{') {
            chars.next();
            let mut name = String::new();
            for next in chars.by_ref() {
                if next == '}' {
                    break;
                }
                name.push(next);
            }
            expanded.push_str(env_value(env, &name).unwrap_or_default());
            continue;
        }

        let mut name = String::new();
        while let Some(next) = chars.peek().copied() {
            if next == '_' || next.is_ascii_alphanumeric() {
                name.push(next);
                chars.next();
            } else {
                break;
            }
        }
        if name.is_empty() {
            expanded.push('$');
        } else {
            expanded.push_str(env_value(env, &name).unwrap_or_default());
        }
    }
    expanded
}

fn env_value<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env.iter()
        .rev()
        .find(|(existing_key, _)| existing_key == key)
        .map(|(_, value)| value.as_str())
}

pub(super) fn ensure_metadata_runtime_dirs(
    env: &[(String, String)],
    host_runtime_dir: &Path,
    uid: u32,
    app_id: &str,
) -> Result<()> {
    let sandbox_runtime_prefix = PathBuf::from(format!("/run/user/{uid}/app/{app_id}"));
    for (key, value) in env {
        if !key.ends_with("_DIR") {
            continue;
        }
        let path = Path::new(value);
        if !path.is_absolute() || !path.starts_with(&sandbox_runtime_prefix) {
            continue;
        }
        let relative = path
            .strip_prefix(format!("/run/user/{uid}"))
            .with_context(|| format!("map sandbox runtime directory {value}"))?;
        let host_path = host_runtime_dir.join(relative);
        fs::create_dir_all(&host_path)
            .with_context(|| format!("create runtime directory {}", host_path.display()))?;
    }
    Ok(())
}

pub(super) fn merge_env(env: &mut Vec<(String, String)>, updates: Vec<(String, String)>) {
    for (key, value) in updates {
        if let Some((_, existing)) = env
            .iter_mut()
            .find(|(existing_key, _)| existing_key == &key)
        {
            *existing = value;
        } else {
            env.push((key, value));
        }
    }
}

pub(super) fn apply_unset_environment(env: &mut Vec<(String, String)>, metadata: &str) {
    let Some(unset) = value(metadata, "Context", "unset-environment") else {
        return;
    };
    let names = unset
        .split(';')
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.starts_with('!'))
        .collect::<Vec<_>>();
    env.retain(|(key, _)| !names.contains(&key.as_str()));
}

pub(super) fn prepend_env_paths(env: &mut Vec<(String, String)>, key: &str, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }

    let prefix = paths.join(":");
    if let Some((_, existing)) = env.iter_mut().find(|(existing_key, _)| existing_key == key) {
        if existing.is_empty() {
            *existing = prefix;
        } else {
            *existing = format!("{prefix}:{existing}");
        }
    } else {
        env.push((key.to_string(), prefix));
    }
}

pub(super) fn apply_graphics_preloads(
    env: &mut Vec<(String, String)>,
    ld_preload_paths: Vec<String>,
    zypak_ld_preload_paths: Vec<String>,
) {
    prepend_env_paths(env, "LD_PRELOAD", ld_preload_paths);
    prepend_env_paths(env, "ZYPAK_LD_PRELOAD", zypak_ld_preload_paths);
}

pub(super) fn app_extension_ld_paths(extensions: &[runtime::AppExtension]) -> Vec<String> {
    extensions
        .iter()
        .filter_map(|extension| {
            extension.ld_library_relative.as_ref().map(|relative| {
                PathBuf::from("/app")
                    .join(&extension.app_mount_relative)
                    .join(relative)
                    .display()
                    .to_string()
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/launch_environment.rs"]
mod tests;
