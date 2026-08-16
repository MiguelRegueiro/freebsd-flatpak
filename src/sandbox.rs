use crate::desktop::DesktopSession;
use crate::linuxulator;
use crate::runtime::FlatpakApp;
use anyhow::{bail, Context, Result};
use std::fs;
use std::os::unix::fs as unix_fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

pub trait SandboxBackend {
    fn run(&self, app: &FlatpakApp, desktop: &DesktopSession) -> Result<ExitStatus>;
}

#[derive(Debug, Clone)]
pub struct ChrootNullfsBackend {
    project_root: PathBuf,
}

impl ChrootNullfsBackend {
    pub fn new(project_root: PathBuf) -> Self {
        Self { project_root }
    }

    fn prepare(&self, app: &FlatpakApp, desktop: &DesktopSession) -> Result<ChrootInstance> {
        let uid = numeric_id("id", "-u")?;
        let gid = numeric_id("id", "-g")?;
        let root = self
            .project_root
            .join("runtime")
            .join("chroots")
            .join(sandbox_name(&app.app_id));

        prepare_root(&root, uid)?;
        let mut instance = ChrootInstance::new(root, uid, gid);

        instance.mount_nullfs(&app.runtime_dir.join("files"), "usr", true)?;
        instance.mount_nullfs(&app.app_dir.join("files"), "app", true)?;
        instance.mount_nullfs(&desktop.xdg_runtime_dir, &format!("run/user/{uid}"), false)?;
        instance.mount_nullfs(Path::new("/tmp"), "tmp", false)?;
        instance.mount_special("dev", "devfs", "devfs")?;
        instance.mount_special("proc", "linprocfs", "linprocfs")?;
        instance.mount_special("sys", "linsysfs", "linsysfs")?;

        Ok(instance)
    }
}

impl SandboxBackend for ChrootNullfsBackend {
    fn run(&self, app: &FlatpakApp, desktop: &DesktopSession) -> Result<ExitStatus> {
        if !desktop.wayland_socket().exists() {
            bail!(
                "Wayland socket does not exist: {}",
                desktop.wayland_socket().display()
            );
        }

        validate_entry(app)?;
        let mut instance = self.prepare(app, desktop)?;
        let status = instance.launch(app, desktop)?;
        instance.cleanup()?;
        Ok(status)
    }
}

#[derive(Debug)]
struct ChrootInstance {
    root: PathBuf,
    uid: u32,
    gid: u32,
    owned_mounts: Vec<PathBuf>,
    cleaned: bool,
}

impl ChrootInstance {
    fn new(root: PathBuf, uid: u32, gid: u32) -> Self {
        Self {
            root,
            uid,
            gid,
            owned_mounts: Vec::new(),
            cleaned: false,
        }
    }

    fn mount_nullfs(
        &mut self,
        source: &Path,
        target_relative: impl AsRef<Path>,
        read_only: bool,
    ) -> Result<()> {
        let source = fs::canonicalize(source)
            .with_context(|| format!("resolve nullfs source {}", source.display()))?;
        let target = self.root.join(target_relative);
        ensure_mountpoint_free(&target)?;

        let mut command = Command::new("doas");
        command.arg("mount_nullfs");
        if read_only {
            command.arg("-o").arg("ro");
        }
        command.arg(&source).arg(&target);
        run_command(command, &format!("mount nullfs {}", target.display()))?;
        self.owned_mounts.push(target);
        Ok(())
    }

    fn mount_special(
        &mut self,
        target_relative: impl AsRef<Path>,
        fs_type: &str,
        source: &str,
    ) -> Result<()> {
        let target = self.root.join(target_relative);
        ensure_mountpoint_free(&target)?;

        let mut command = Command::new("doas");
        command
            .arg("mount")
            .arg("-t")
            .arg(fs_type)
            .arg(source)
            .arg(&target);
        run_command(command, &format!("mount {fs_type} {}", target.display()))?;
        self.owned_mounts.push(target);
        Ok(())
    }

    fn launch(&self, app: &FlatpakApp, desktop: &DesktopSession) -> Result<ExitStatus> {
        let user = std::env::var("USER").unwrap_or_else(|_| self.uid.to_string());
        let entry = chroot_entry_path(&app.command);
        let env = launch_env(app, desktop, self.uid, &user);

        eprintln!("launching {} from {}", app.app_id, self.root.display());
        eprintln!(
            "  runtime: {} ({})",
            app.runtime_ref,
            app.runtime_dir.display()
        );
        eprintln!("  app: {}", app.app_dir.display());
        eprintln!("  entry: /lib64/ld-linux-x86-64.so.2 {entry}");

        let mut command = Command::new("doas");
        command
            .arg("chroot")
            .arg("-u")
            .arg(self.uid.to_string())
            .arg("-g")
            .arg(self.gid.to_string())
            .arg(&self.root)
            .arg("/usr/bin/env")
            .arg("-i");
        for (key, value) in env {
            command.arg(format!("{key}={value}"));
        }
        command
            .arg("/lib64/ld-linux-x86-64.so.2")
            .arg(entry)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        command.status().context("launch app through chroot")
    }

    fn cleanup(&mut self) -> Result<()> {
        let mut errors = Vec::new();
        for mountpoint in self.owned_mounts.iter().rev() {
            let mut command = Command::new("doas");
            command.arg("umount").arg(mountpoint);
            if let Err(error) = run_command(command, &format!("umount {}", mountpoint.display())) {
                errors.push(format!("{error:#}"));
            }
        }
        self.cleaned = true;

        if errors.is_empty() {
            Ok(())
        } else {
            bail!("cleanup failed:\n{}", errors.join("\n"));
        }
    }
}

impl Drop for ChrootInstance {
    fn drop(&mut self) {
        if self.cleaned {
            return;
        }
        if let Err(error) = self.cleanup() {
            eprintln!("warning: sandbox cleanup failed: {error:#}");
        }
    }
}

fn prepare_root(root: &Path, uid: u32) -> Result<()> {
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
    fs::create_dir_all(root.join("run").join("user").join(uid.to_string()))
        .with_context(|| format!("create {}", root.join("run/user").display()))?;

    make_link("usr/bin", &root.join("bin"))?;
    make_link("usr/lib", &root.join("lib"))?;
    make_link("usr/lib64", &root.join("lib64"))?;
    make_link("usr/etc", &root.join("etc"))?;
    Ok(())
}

fn make_link(target: &str, link: &Path) -> Result<()> {
    if fs::symlink_metadata(link).is_ok() {
        return Ok(());
    }
    unix_fs::symlink(target, link)
        .with_context(|| format!("symlink {} -> {target}", link.display()))
}

fn launch_env(
    app: &FlatpakApp,
    desktop: &DesktopSession,
    uid: u32,
    user: &str,
) -> Vec<(String, String)> {
    let mut env = vec![
        ("HOME".to_string(), "/var/data".to_string()),
        ("USER".to_string(), user.to_string()),
        ("LOGNAME".to_string(), user.to_string()),
        ("SHELL".to_string(), "/bin/sh".to_string()),
        ("container".to_string(), "flatpak".to_string()),
        ("FLATPAK_ID".to_string(), app.app_id.clone()),
        ("XDG_RUNTIME_DIR".to_string(), format!("/run/user/{uid}")),
        ("WAYLAND_DISPLAY".to_string(), desktop.wayland_display.clone()),
        ("LANG".to_string(), "C.UTF-8".to_string()),
        ("LC_ALL".to_string(), "C.UTF-8".to_string()),
        ("GDK_BACKEND".to_string(), "wayland".to_string()),
        ("GSK_RENDERER".to_string(), "cairo".to_string()),
        ("GTK_USE_PORTAL".to_string(), "0".to_string()),
        ("XDG_DATA_HOME".to_string(), "/var/data".to_string()),
        ("XDG_CONFIG_HOME".to_string(), "/var/config".to_string()),
        ("XDG_CACHE_HOME".to_string(), "/var/cache".to_string()),
        ("XDG_CONFIG_DIRS".to_string(), "/app/etc/xdg:/etc/xdg".to_string()),
        (
            "XDG_DATA_DIRS".to_string(),
            "/app/share:/usr/share:/usr/share/runtime/share".to_string(),
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
    if let Some(address) = desktop.chroot_dbus_address(uid) {
        env.push(("DBUS_SESSION_BUS_ADDRESS".to_string(), address));
    }

    env
}

fn validate_entry(app: &FlatpakApp) -> Result<()> {
    let host_entry = if app.command.starts_with('/') {
        app.app_dir
            .join("files")
            .join(app.command.trim_start_matches('/'))
    } else {
        app.app_dir.join("files").join("bin").join(&app.command)
    };
    if !host_entry.exists() {
        bail!("entry executable does not exist: {}", host_entry.display());
    }
    if !linuxulator::is_linux_elf(&host_entry) {
        eprintln!(
            "warning: entry is not a Linux ELF according to the shallow probe: {}",
            host_entry.display()
        );
    }
    Ok(())
}

fn chroot_entry_path(command: &str) -> String {
    if command.starts_with('/') {
        command.to_string()
    } else {
        format!("/app/bin/{command}")
    }
}

fn sandbox_name(app_id: &str) -> String {
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

fn numeric_id(program: &str, arg: &str) -> Result<u32> {
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

fn ensure_mountpoint_free(target: &Path) -> Result<()> {
    if is_mounted(target)? {
        bail!(
            "sandbox mountpoint is already mounted; clean it first: {}",
            target.display()
        );
    }
    Ok(())
}

fn is_mounted(target: &Path) -> Result<bool> {
    let output = Command::new("mount").output().context("read mount table")?;
    if !output.status.success() {
        bail!("mount command failed with status {}", output.status);
    }
    let target = target.to_string_lossy();
    let mount_table = String::from_utf8(output.stdout)?;
    Ok(mount_table.lines().any(|line| {
        line.split_once(" on ")
            .and_then(|(_, rest)| rest.split_once(" ("))
            .map(|(mountpoint, _)| mountpoint == target)
            .unwrap_or(false)
    }))
}

fn run_command(mut command: Command, action: &str) -> Result<()> {
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
