use crate::audio::HostAudio;
use crate::desktop::DesktopSession;
use crate::filesystem::HostFilesystem;
use crate::linuxulator;
use crate::runtime::FlatpakApp;
use crate::state;
use anyhow::{bail, Context, Result};
use std::fs;
use std::os::unix::fs as unix_fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::thread;
use std::time::Duration;

static SIGNAL_HANDLERS_INSTALLED: AtomicBool = AtomicBool::new(false);
static ACTIVE_CHILD_PID: AtomicI32 = AtomicI32::new(0);
static LAST_SIGNAL: AtomicI32 = AtomicI32::new(0);

pub trait SandboxBackend {
    fn run(&self, app: &FlatpakApp, desktop: &DesktopSession) -> Result<ExitStatus>;
}

pub fn recover_stale_mounts(project_root: &Path) -> Result<()> {
    state::ensure_layout(project_root)?;
    let mut active_roots = Vec::new();

    for record in state::read_run_records(project_root)? {
        let Some(record_path) = record.get("_path").map(PathBuf::from) else {
            continue;
        };
        let launcher_pid = record
            .get("launcher_pid")
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0);
        let child_pid = record
            .get("child_pid")
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0);
        let root = record
            .get("root")
            .map(|path| state::absolute(project_root, Path::new(path)));

        if launcher_pid > 0 && process_alive(launcher_pid) {
            if let Some(root) = root {
                active_roots.push(root);
            }
            continue;
        }

        if child_pid > 0 && process_alive(child_pid) {
            eprintln!("recovering stale sandbox child pid {child_pid}");
            terminate_process(child_pid);
        }
        if let Some(root) = root {
            unmount_under(&root)?;
        }
        state::remove_run_record(&record_path)?;
    }

    let chroot_root = project_root.join("runtime").join("chroots");
    let mut stale_mounts = mount_points_under(&chroot_root)?;
    stale_mounts.retain(|mountpoint| !active_roots.iter().any(|root| mountpoint.starts_with(root)));
    unmount_mountpoints(stale_mounts)?;
    Ok(())
}

pub fn app_has_mounts(project_root: &Path, app_id: &str) -> Result<bool> {
    let root = project_root
        .join("runtime")
        .join("chroots")
        .join(sandbox_name(app_id));
    Ok(!mount_points_under(&root)?.is_empty())
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
        let user = host_user(uid);
        let root = self
            .project_root
            .join("runtime")
            .join("chroots")
            .join(sandbox_name(&app.app_id));
        let host_filesystem = HostFilesystem::from_metadata_file_for_user(
            &app.app_dir.join("metadata"),
            &user,
            &self.project_root,
            &root,
        )?;
        let host_audio = HostAudio::from_metadata_file(
            &app.app_dir.join("metadata"),
            &desktop.xdg_runtime_dir,
            uid,
        )?;

        prepare_root(&root, uid)?;
        host_filesystem.write_xdg_user_dirs_config(&root)?;
        host_audio.prepare(&root)?;
        let mut instance = ChrootInstance::new(
            self.project_root.clone(),
            app.app_id.clone(),
            root,
            uid,
            gid,
            host_filesystem,
            host_audio,
        );

        instance.mount_nullfs(&app.runtime_dir.join("files"), "usr", true)?;
        instance.mount_nullfs(&app.app_dir.join("files"), "app", true)?;
        for grant in instance.host_filesystem.grants().to_vec() {
            instance.mount_nullfs(
                grant.host_path(),
                grant.sandbox_target_relative()?,
                grant.access().is_read_only(),
            )?;
        }
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
        install_signal_handlers();
        if !desktop.wayland_socket().exists() {
            bail!(
                "Wayland socket does not exist: {}",
                desktop.wayland_socket().display()
            );
        }

        let entry = resolve_entry(app)?;
        let mut instance = self.prepare(app, desktop)?;
        let status = instance.launch(app, desktop, &entry)?;
        instance.cleanup()?;
        Ok(status)
    }
}

#[derive(Debug)]
struct ChrootInstance {
    project_root: PathBuf,
    app_id: String,
    root: PathBuf,
    uid: u32,
    gid: u32,
    host_filesystem: HostFilesystem,
    host_audio: HostAudio,
    owned_mounts: Vec<OwnedMount>,
    run_record: Option<PathBuf>,
    cleaned: bool,
}

#[derive(Debug)]
struct OwnedMount {
    path: PathBuf,
    read_only: bool,
}

impl ChrootInstance {
    fn new(
        project_root: PathBuf,
        app_id: String,
        root: PathBuf,
        uid: u32,
        gid: u32,
        host_filesystem: HostFilesystem,
        host_audio: HostAudio,
    ) -> Self {
        Self {
            project_root,
            app_id,
            root,
            uid,
            gid,
            host_filesystem,
            host_audio,
            owned_mounts: Vec::new(),
            run_record: None,
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
        fs::create_dir_all(&target)
            .with_context(|| format!("create mount target {}", target.display()))?;
        ensure_mountpoint_free(&target)?;

        let mut command = Command::new("doas");
        command.arg("mount_nullfs");
        if read_only {
            command.arg("-o").arg("ro");
        }
        command.arg(&source).arg(&target);
        run_command(command, &format!("mount nullfs {}", target.display()))?;
        self.owned_mounts.push(OwnedMount {
            path: target,
            read_only,
        });
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
        self.owned_mounts.push(OwnedMount {
            path: target,
            read_only: false,
        });
        Ok(())
    }

    fn launch(
        &mut self,
        app: &FlatpakApp,
        desktop: &DesktopSession,
        entry: &EntryLaunch,
    ) -> Result<ExitStatus> {
        install_signal_handlers();
        let user = host_user(self.uid);
        let mut env = launch_env(app, desktop, self.uid, &user);
        env.retain(|(key, _)| key != "HOME");
        env.push((
            "HOME".to_string(),
            self.host_filesystem.sandbox_home_env("/var/data"),
        ));
        env.extend(self.host_filesystem.user_dir_env());
        env.extend(self.host_audio.env());
        let app_args = self.host_filesystem.translate_args(&app.args)?;

        eprintln!("launching {} from {}", app.app_id, self.root.display());
        eprintln!(
            "  runtime: {} ({})",
            app.runtime_ref,
            app.runtime_dir.display()
        );
        eprintln!("  app: {}", app.app_dir.display());
        for grant in self.host_filesystem.grants() {
            eprintln!(
                "  host fs: {} -> {} ({}, from {})",
                grant.host_path().display(),
                grant.sandbox_path().display(),
                grant.access().label(),
                grant.source_permission()
            );
        }
        for warning in self.host_filesystem.warnings() {
            eprintln!("  host fs warning: {warning}");
        }
        for line in self.host_audio.describe() {
            eprintln!("  audio: {line}");
        }
        for warning in self.host_audio.warnings() {
            eprintln!("  audio warning: {warning}");
        }
        eprintln!("  entry: {}", entry.display(&app_args));

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
        entry.append_command_args(&mut command, &app_args);
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        LAST_SIGNAL.store(0, Ordering::SeqCst);
        let mut child = command.spawn().context("launch app through chroot")?;
        ACTIVE_CHILD_PID.store(child.id() as i32, Ordering::SeqCst);
        self.run_record = Some(state::write_run_record(
            &self.project_root,
            &self.app_id,
            &self.root,
            std::process::id(),
            child.id(),
        )?);
        let status = child.wait().context("wait for app process")?;
        ACTIVE_CHILD_PID.store(0, Ordering::SeqCst);

        let signal = LAST_SIGNAL.swap(0, Ordering::SeqCst);
        if signal != 0 {
            eprintln!("received signal {signal}; app process exited, cleaning up sandbox");
        }

        Ok(status)
    }

    fn cleanup(&mut self) -> Result<()> {
        let mut errors = Vec::new();
        for mount in self.owned_mounts.iter().rev() {
            if let Err(error) =
                unmount_mountpoint(&mount.path, mount.read_only, "umount owned mount")
            {
                errors.push(format!("{error:#}"));
            }
        }
        if let Err(error) = self.host_audio.cleanup(&self.root) {
            errors.push(format!("{error:#}"));
        }

        if errors.is_empty() {
            if let Some(path) = &self.run_record {
                state::remove_run_record(path)?;
            }
            self.cleaned = true;
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

#[derive(Debug, Clone)]
struct EntryLaunch {
    chroot_path: String,
    mode: EntryLaunchMode,
}

#[derive(Debug, Clone)]
enum EntryLaunchMode {
    LinuxElf,
    Direct,
}

impl EntryLaunch {
    fn display(&self, args: &[String]) -> String {
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

    fn append_command_args(&self, command: &mut Command, args: &[String]) {
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

fn resolve_entry(app: &FlatpakApp) -> Result<EntryLaunch> {
    let app_files = app.app_dir.join("files");
    let chroot_path = chroot_entry_path(&app.command);
    let host_entry = host_app_path(&app_files, &chroot_path)?;
    if fs::symlink_metadata(&host_entry).is_err() {
        bail!("entry executable does not exist: {}", host_entry.display());
    }
    let probe_entry =
        resolve_app_symlink_for_probe(&app_files, &host_entry).unwrap_or(host_entry.clone());
    let mode = if linuxulator::is_linux_elf(&probe_entry) {
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

fn host_user(uid: u32) -> String {
    std::env::var("USER").unwrap_or_else(|_| uid.to_string())
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
    Ok(mount_points()?
        .iter()
        .any(|mountpoint| mountpoint == target))
}

fn mount_points_under(root: &Path) -> Result<Vec<PathBuf>> {
    let mut mounts: Vec<PathBuf> = mount_points()?
        .into_iter()
        .filter(|mountpoint| mountpoint.starts_with(root))
        .collect();
    mounts.sort_by(|left, right| {
        right
            .components()
            .count()
            .cmp(&left.components().count())
            .then_with(|| right.cmp(left))
    });
    Ok(mounts)
}

fn mount_points() -> Result<Vec<PathBuf>> {
    Ok(mount_infos()?
        .into_iter()
        .map(|mount| mount.mountpoint)
        .collect())
}

#[derive(Debug)]
struct MountInfo {
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
                .and_then(|(_, rest)| rest.split_once(" ("))
                .map(|(mountpoint, options)| MountInfo {
                    mountpoint: PathBuf::from(mountpoint),
                    options: options.trim_end_matches(')').to_string(),
                })
        })
        .collect())
}

fn unmount_under(root: &Path) -> Result<()> {
    unmount_mountpoints(mount_points_under(root)?)
}

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

fn unmount_mountpoint(mountpoint: &Path, allow_force: bool, action: &str) -> Result<()> {
    let mut last_error = None;
    for _ in 0..8 {
        let mut command = Command::new("doas");
        command.arg("umount").arg(mountpoint);
        match run_command(command, &format!("{action} {}", mountpoint.display())) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(250));
            }
        }
    }

    if allow_force {
        eprintln!(
            "warning: normal unmount stayed busy for read-only mount {}; trying umount -f",
            mountpoint.display()
        );
        let mut command = Command::new("doas");
        command.arg("umount").arg("-f").arg(mountpoint);
        run_command(command, &format!("{action} -f {}", mountpoint.display()))?;
        return Ok(());
    }

    let Some(error) = last_error else {
        bail!("{action} {} was not attempted", mountpoint.display());
    };
    Err(error)
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

fn install_signal_handlers() {
    if SIGNAL_HANDLERS_INSTALLED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    unsafe {
        libc::signal(
            libc::SIGINT,
            handle_signal as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGTERM,
            handle_signal as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGHUP,
            handle_signal as *const () as libc::sighandler_t,
        );
    }
}

extern "C" fn handle_signal(signal: libc::c_int) {
    LAST_SIGNAL.store(signal, Ordering::SeqCst);
    if signal == libc::SIGHUP {
        return;
    }

    let pid = ACTIVE_CHILD_PID.load(Ordering::SeqCst);
    if pid > 0 {
        unsafe {
            libc::kill(pid, signal);
        }
    }
}

fn process_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

fn terminate_process(pid: i32) {
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
    for _ in 0..20 {
        if !process_alive(pid) {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }

    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
    for _ in 0..20 {
        if !process_alive(pid) {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
}
