use crate::paths::Installation;
use crate::state::{self, AppRecord};
use anyhow::{bail, Context, Result};
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs as unix_fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Default, Clone)]
pub struct ExportReport {
    pub files: usize,
    pub desktop_entries: usize,
    pub skipped: Vec<PathBuf>,
}

pub fn export_data_dir(paths: &Installation) -> PathBuf {
    paths.export_share()
}

pub fn export_app(paths: &Installation, app: &AppRecord) -> Result<ExportReport> {
    remove_export_files(paths, &app.app_id)?;

    let source_share = state::absolute(paths, &app.app_dir)
        .join("export")
        .join("share");
    let mut report = ExportReport::default();
    if !source_share.is_dir() {
        eprintln!(
            "warning: {} has no exported desktop data at {}",
            app.app_id,
            source_share.display()
        );
        return Ok(report);
    }

    let export_share = export_data_dir(paths);
    fs::create_dir_all(&export_share)
        .with_context(|| format!("create export data dir {}", export_share.display()))?;

    let flatpak_bin = paths.launcher();
    if !flatpak_bin.is_file() {
        eprintln!(
            "warning: exported desktop entries will call {}, but it does not exist yet",
            flatpak_bin.display()
        );
    }

    let mut exported_paths = Vec::new();
    copy_export_dir(
        &source_share,
        &source_share,
        &export_share,
        flatpak_bin,
        &app.app_id,
        &mut exported_paths,
        &mut report,
    )?;
    exported_paths.sort();
    for rel in &exported_paths {
        publish_projection(paths, rel)?;
    }
    state::write_export_record(paths, &app.app_id, &exported_paths)?;
    refresh_export_caches(paths)?;

    report.files = exported_paths.len();
    Ok(report)
}

pub fn remove_export(paths: &Installation, app_id: &str) -> Result<()> {
    remove_export_files(paths, app_id)?;
    refresh_export_caches(paths)
}

fn remove_export_files(paths: &Installation, app_id: &str) -> Result<()> {
    let export_share = export_data_dir(paths);
    let mut parents = Vec::new();

    for rel in state::read_export_record(paths, app_id)? {
        validate_relative_export_path(&rel)?;
        let target = export_share.join(&rel);
        remove_projection(paths, &rel, &target)?;
        if let Some(parent) = target.parent() {
            parents.push(parent.to_path_buf());
        }

        let Ok(metadata) = fs::symlink_metadata(&target) else {
            continue;
        };
        if metadata.file_type().is_dir() {
            bail!(
                "refusing to remove directory listed as exported file: {}",
                target.display()
            );
        }
        fs::remove_file(&target).with_context(|| format!("remove {}", target.display()))?;
    }

    parents.sort();
    parents.dedup();
    for parent in parents.into_iter().rev() {
        remove_empty_parents(&export_share, &parent)?;
    }

    state::remove_export_record(paths, app_id)?;
    Ok(())
}

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

fn publish_projection(paths: &Installation, rel: &Path) -> Result<()> {
    if !is_launcher_projection(rel) {
        return Ok(());
    }
    let source = paths.export_share().join(rel);
    let target = paths.data_home().join(rel);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create XDG export directory {}", parent.display()))?;
    }

    if let Ok(metadata) = fs::symlink_metadata(&target) {
        if !metadata.file_type().is_symlink()
            || fs::read_link(&target).ok().as_deref() != Some(source.as_path())
        {
            bail!(
                "refusing to replace existing XDG export {} (source would be {})",
                target.display(),
                source.display()
            );
        }
        fs::remove_file(&target).with_context(|| format!("replace {}", target.display()))?;
    }
    unix_fs::symlink(&source, &target)
        .with_context(|| format!("publish XDG export {}", target.display()))
}

fn remove_projection(paths: &Installation, rel: &Path, source: &Path) -> Result<()> {
    if !is_launcher_projection(rel) {
        return Ok(());
    }
    let target = paths.data_home().join(rel);
    let Ok(metadata) = fs::symlink_metadata(&target) else {
        return Ok(());
    };
    if metadata.file_type().is_symlink() && fs::read_link(&target).ok().as_deref() == Some(source) {
        fs::remove_file(&target).with_context(|| format!("remove {}", target.display()))?;
        if let Some(parent) = target.parent() {
            remove_empty_parents(paths.data_home(), parent)?;
        }
    }
    Ok(())
}

fn is_launcher_projection(rel: &Path) -> bool {
    matches!(
        rel.components().next(),
        Some(Component::Normal(name))
            if name == "applications" || name == "icons" || name == "metainfo" || name == "appdata"
    )
}

fn copy_export_dir(
    source_root: &Path,
    source_dir: &Path,
    export_share: &Path,
    flatpak_bin: &Path,
    app_id: &str,
    exported_paths: &mut Vec<PathBuf>,
    report: &mut ExportReport,
) -> Result<()> {
    let mut entries = fs::read_dir(source_dir)
        .with_context(|| format!("read export directory {}", source_dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let source = entry.path();
        let rel = source
            .strip_prefix(source_root)
            .with_context(|| format!("make {} relative", source.display()))?
            .to_path_buf();
        validate_relative_export_path(&rel)?;

        if should_skip_export_path(&rel) {
            if rel.components().count() == 1 {
                report.skipped.push(rel);
            }
            continue;
        }

        let target = export_share.join(&rel);
        let metadata =
            fs::symlink_metadata(&source).with_context(|| format!("stat {}", source.display()))?;
        let file_type = metadata.file_type();

        if file_type.is_dir() {
            fs::create_dir_all(&target)
                .with_context(|| format!("create export dir {}", target.display()))?;
            copy_export_dir(
                source_root,
                &source,
                export_share,
                flatpak_bin,
                app_id,
                exported_paths,
                report,
            )?;
            continue;
        }

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create export dir {}", parent.display()))?;
        }
        remove_existing_export_file(&target)?;

        if file_type.is_symlink() {
            let link_target = fs::read_link(&source)
                .with_context(|| format!("read link {}", source.display()))?;
            unix_fs::symlink(&link_target, &target)
                .with_context(|| format!("symlink {}", target.display()))?;
        } else if file_type.is_file() {
            if is_desktop_file(&rel) {
                rewrite_desktop_file(&source, &target, flatpak_bin, app_id)?;
                report.desktop_entries += 1;
            } else {
                fs::copy(&source, &target).with_context(|| {
                    format!("copy {} to {}", source.display(), target.display())
                })?;
            }
            fs::set_permissions(&target, metadata.permissions())
                .with_context(|| format!("set permissions on {}", target.display()))?;
        } else {
            eprintln!(
                "warning: skipping unsupported export file type {}",
                source.display()
            );
            continue;
        }

        exported_paths.push(rel);
    }

    Ok(())
}

fn rewrite_desktop_file(
    source: &Path,
    target: &Path,
    flatpak_bin: &Path,
    app_id: &str,
) -> Result<()> {
    let data = fs::read_to_string(source)
        .with_context(|| format!("read desktop file {}", source.display()))?;
    let mut rewritten = String::with_capacity(data.len() + 128);

    for line in data.lines() {
        if let Some(exec) = line.strip_prefix("Exec=") {
            rewritten.push_str("Exec=");
            rewritten.push_str(&desktop_exec(flatpak_bin, app_id, exec));
        } else if line == "DBusActivatable=true" {
            rewritten.push_str("DBusActivatable=false");
        } else if line.starts_with("TryExec=") {
            rewritten.push_str("TryExec=");
            rewritten.push_str(&desktop_quote_arg(&flatpak_bin.display().to_string()));
        } else {
            rewritten.push_str(line);
        }
        rewritten.push('\n');
    }

    fs::write(target, rewritten).with_context(|| format!("write {}", target.display()))
}

fn desktop_exec(flatpak_bin: &Path, app_id: &str, original: &str) -> String {
    let tail = exec_tail_after_command(original);
    let mut exec = format!(
        "{} run {}",
        desktop_quote_arg(&flatpak_bin.display().to_string()),
        desktop_quote_arg(app_id)
    );
    if !tail.is_empty() {
        exec.push_str(" -- ");
        exec.push_str(tail);
    }
    exec
}

fn exec_tail_after_command(original: &str) -> &str {
    let trimmed = original.trim_start();
    if trimmed.is_empty() {
        return "";
    }

    let mut chars = trimmed.char_indices();
    let Some((_, first)) = chars.next() else {
        return "";
    };

    if first == '"' || first == '\'' {
        let quote = first;
        let mut escaped = false;
        for (idx, ch) in chars {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == quote {
                return trimmed[idx + ch.len_utf8()..].trim_start();
            }
        }
        return "";
    }

    let mut escaped = false;
    for (idx, ch) in trimmed.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch.is_whitespace() {
            return trimmed[idx..].trim_start();
        }
    }

    ""
}

fn desktop_quote_arg(arg: &str) -> String {
    if arg
        .chars()
        .all(|ch| !ch.is_whitespace() && !matches!(ch, '"' | '\\' | '$' | '`'))
    {
        return arg.to_string();
    }

    let mut quoted = String::from("\"");
    for ch in arg.chars() {
        if matches!(ch, '"' | '\\' | '$' | '`') {
            quoted.push('\\');
        }
        quoted.push(ch);
    }
    quoted.push('"');
    quoted
}

fn should_skip_export_path(rel: &Path) -> bool {
    matches!(
        rel.components().next(),
        Some(Component::Normal(name)) if name == "dbus-1" || name == "gnome-shell"
    )
}

fn is_desktop_file(rel: &Path) -> bool {
    rel.extension().and_then(|ext| ext.to_str()) == Some("desktop")
}

fn validate_relative_export_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        bail!("invalid export path: {}", path.display());
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => {}
            _ => bail!("invalid export path: {}", path.display()),
        }
    }
    Ok(())
}

fn remove_existing_export_file(path: &Path) -> Result<()> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if metadata.file_type().is_dir() {
        bail!("refusing to replace export directory {}", path.display());
    }
    fs::remove_file(path).with_context(|| format!("remove old export {}", path.display()))
}

fn remove_empty_parents(root: &Path, leaf: &Path) -> Result<()> {
    let mut current = leaf.to_path_buf();
    while current.starts_with(root) && current != root {
        match fs::remove_dir(&current) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) if error.kind() == ErrorKind::DirectoryNotEmpty => break,
            Err(error) => {
                return Err(error).with_context(|| format!("remove {}", current.display()))
            }
        }
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent.to_path_buf();
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

#[derive(Debug, Clone)]
pub struct DesktopSession {
    pub xdg_runtime_dir: PathBuf,
    pub wayland_display: String,
    pub display: Option<String>,
    pub dbus_session_bus_address: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{desktop_exec, exec_tail_after_command, export_app, remove_export};
    use crate::paths::Installation;
    use crate::state::AppRecord;
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn rewrites_simple_exec() {
        assert_eq!(
            desktop_exec(
                Path::new("/poc/bin/flatpak"),
                "org.example.App",
                "app-binary"
            ),
            "/poc/bin/flatpak run org.example.App"
        );
    }

    #[test]
    fn preserves_desktop_exec_arguments() {
        assert_eq!(
            desktop_exec(
                Path::new("/poc/bin/flatpak"),
                "org.example.App",
                "app-binary --new-window %U"
            ),
            "/poc/bin/flatpak run org.example.App -- --new-window %U"
        );
    }

    #[test]
    fn handles_quoted_exec_command() {
        assert_eq!(
            exec_tail_after_command("\"/app/bin/example command\" %F"),
            "%F"
        );
    }

    #[test]
    fn publishes_desktop_files_into_normal_xdg_data_home() {
        let root = std::env::temp_dir().join(format!(
            "freebsd-flatpak-desktop-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let paths = Installation::for_test(&root);
        paths.ensure().unwrap();
        let source = paths
            .app("org.example.App")
            .join("export/share/applications/org.example.App.desktop");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(
            &source,
            "[Desktop Entry]\nName=Example\nExec=example %U\nDBusActivatable=true\n",
        )
        .unwrap();
        let app = AppRecord {
            app_id: "org.example.App".into(),
            app_ref: "app/org.example.App/x86_64/stable".into(),
            app_commit: "a".repeat(64),
            app_dir: PathBuf::from("apps/org.example.App"),
            arch: "x86_64".into(),
            branch: "stable".into(),
            runtime_ref: "org.example.Platform/x86_64/stable".into(),
            runtime_commit: "b".repeat(64),
            runtime_dir: PathBuf::from("runtimes/org.example.Platform-stable"),
            command: "example".into(),
        };

        export_app(&paths, &app).unwrap();
        let projected = paths
            .data_home()
            .join("applications/org.example.App.desktop");
        assert!(fs::symlink_metadata(&projected)
            .unwrap()
            .file_type()
            .is_symlink());
        let desktop = fs::read_to_string(&projected).unwrap();
        assert!(desktop.contains("Exec=/usr/local/bin/flatpak run org.example.App -- %U"));
        assert!(!desktop.contains(root.to_str().unwrap()));

        remove_export(&paths, &app.app_id).unwrap();
        assert!(!projected.exists());
        let _ = fs::remove_dir_all(root);
    }
}

impl DesktopSession {
    pub fn from_env() -> Option<Self> {
        Some(Self {
            xdg_runtime_dir: std::env::var_os("XDG_RUNTIME_DIR")?.into(),
            wayland_display: std::env::var("WAYLAND_DISPLAY").ok()?,
            display: std::env::var("DISPLAY").ok(),
            dbus_session_bus_address: std::env::var("DBUS_SESSION_BUS_ADDRESS").ok(),
        })
    }

    pub fn wayland_socket(&self) -> PathBuf {
        self.xdg_runtime_dir.join(&self.wayland_display)
    }

    pub fn chroot_dbus_address(&self, uid: u32) -> Option<String> {
        let address = self.dbus_session_bus_address.as_ref()?;
        let path = address.strip_prefix("unix:path=")?;
        let host_path = PathBuf::from(path);

        if let Ok(relative) = host_path.strip_prefix(&self.xdg_runtime_dir) {
            return Some(format!(
                "unix:path=/run/user/{}/{}",
                uid,
                relative.display()
            ));
        }

        Some(address.clone())
    }
}
