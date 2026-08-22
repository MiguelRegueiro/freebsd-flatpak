use crate::audio::HostAudio;
use crate::cursor::HostCursorTheme;
use crate::desktop::DesktopSession;
use crate::filesystem::HostFilesystem;
use crate::fonts::HostFonts;
use crate::graphics::HostGraphics;
use crate::linuxulator;
use crate::paths::Installation;
use crate::portal::HostPortal;
use crate::runtime::{self, FlatpakApp};
use crate::state;
use crate::video::HostVideo;
use anyhow::{bail, Context, Result};
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs as unix_fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static SIGNAL_HANDLERS_INSTALLED: AtomicBool = AtomicBool::new(false);
static ACTIVE_CHILD_PID: AtomicI32 = AtomicI32::new(0);
static LAST_SIGNAL: AtomicI32 = AtomicI32::new(0);
static INSTANCE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub trait SandboxBackend {
    fn run(&self, app: &FlatpakApp, desktop: &DesktopSession) -> Result<ExitStatus>;
}

pub fn recover_stale_mounts(paths: &Installation) -> Result<()> {
    state::ensure_layout(paths)?;

    for record in state::read_run_records(paths)? {
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
        let root = record.get("root").map(PathBuf::from);

        if launcher_pid > 0 && process_alive(launcher_pid) {
            continue;
        }

        if child_pid > 0 && process_alive(child_pid) {
            eprintln!("recovering stale sandbox child pid {child_pid}");
            terminate_process(child_pid);
        }
        if let Some(root) = root {
            terminate_chroot_processes(&root)?;
            unmount_under(&root)?;
            remove_instance_root(&root)?;
        }
        state::remove_run_record(&record_path)?;
    }

    // Refresh active roots after observing the mount table. A concurrent run
    // publishes its record before creating any mounts, so every mount that can
    // appear here has an ownership record visible in this second snapshot.
    let chroot_root = paths.chroots();
    let mut stale_mounts = mount_points_under(&chroot_root)?;
    let active_roots = active_run_roots(paths)?;
    stale_mounts.retain(|mountpoint| !active_roots.iter().any(|root| mountpoint.starts_with(root)));
    let mut stale_roots = BTreeSet::new();
    for mountpoint in &stale_mounts {
        if let Some(root) = chroot_root_for_mount(&chroot_root, mountpoint) {
            stale_roots.insert(root);
        }
    }
    for root in stale_roots {
        terminate_chroot_processes(&root)?;
        terminate_chroot_mount_holders(&root)?;
    }
    let mut stale_mounts = mount_points_under(&chroot_root)?;
    stale_mounts.retain(|mountpoint| !active_roots.iter().any(|root| mountpoint.starts_with(root)));
    unmount_mountpoints(stale_mounts)?;
    Ok(())
}

pub fn app_has_mounts(paths: &Installation, app_id: &str) -> Result<bool> {
    let root = paths.chroots().join(sandbox_name(app_id));
    Ok(!mount_points_under(&root)?.is_empty())
}

fn active_run_roots(paths: &Installation) -> Result<Vec<PathBuf>> {
    Ok(state::read_run_records(paths)?
        .into_iter()
        .filter(|record| {
            record
                .get("launcher_pid")
                .and_then(|value| value.parse::<i32>().ok())
                .is_some_and(|pid| pid > 0 && process_alive(pid))
        })
        .filter_map(|record| record.get("root").map(PathBuf::from))
        .collect())
}

fn new_instance_id() -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = INSTANCE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    // Keep the launcher PID as the final component for compatibility with
    // existing transient-resource stale recovery.
    format!("{nonce:x}-{sequence:x}-{}", std::process::id())
}

struct PendingRunRecord {
    path: PathBuf,
    committed: bool,
}

impl PendingRunRecord {
    fn new(paths: &Installation, app_id: &str, instance_id: &str, root: &Path) -> Result<Self> {
        let path =
            state::write_run_record(paths, app_id, instance_id, root, std::process::id(), 0)?;
        Ok(Self {
            path,
            committed: false,
        })
    }

    fn commit(mut self) -> PathBuf {
        self.committed = true;
        self.path.clone()
    }
}

impl Drop for PendingRunRecord {
    fn drop(&mut self) {
        if !self.committed {
            if let Err(error) = state::remove_run_record(&self.path) {
                eprintln!("warning: remove uncommitted run record failed: {error:#}");
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChrootNullfsBackend {
    paths: Installation,
}

impl ChrootNullfsBackend {
    pub fn new(paths: Installation) -> Self {
        Self { paths }
    }

    fn prepare(&self, app: &FlatpakApp, desktop: &DesktopSession) -> Result<ChrootInstance> {
        let uid = numeric_id("id", "-u")?;
        let gid = numeric_id("id", "-g")?;
        let supplementary_gids = numeric_ids("id", "-G")?;
        let user = host_user(uid);
        let instance_id = new_instance_id();
        let root = self
            .paths
            .chroots()
            .join(sandbox_name(&app.app_id))
            .join(&instance_id);
        let pending_run = PendingRunRecord::new(&self.paths, &app.app_id, &instance_id, &root)?;
        let metadata_path = app.app_dir.join("metadata");
        let network_enabled = app_allows_network(&metadata_path)?;
        let host_filesystem = HostFilesystem::from_metadata_file_for_user(
            &metadata_path,
            &user,
            self.paths.data_root(),
            &root,
        )?;
        let host_audio =
            HostAudio::from_metadata_file(&metadata_path, &desktop.xdg_runtime_dir, uid)?;
        let host_cursor = HostCursorTheme::from_host();
        let host_fonts = HostFonts::from_host();
        let host_portal =
            HostPortal::prepare(&self.paths, &app.app_id, &instance_id, desktop, uid, &root)?;
        let host_graphics = HostGraphics::prepare(&self.paths, app, &instance_id)?;
        let host_video = HostVideo::prepare(&self.paths, app)?;
        let app_extensions = runtime::ensure_app_codec_extensions(&self.paths, app)?;

        prepare_root(
            &root,
            uid,
            &app.runtime_dir.join("files").join("etc"),
            network_enabled,
        )?;
        write_flatpak_info(&root, app, &instance_id)?;
        host_filesystem.write_xdg_user_dirs_config(&root)?;
        host_audio.prepare(&root)?;
        host_fonts.prepare(&root)?;
        let mut instance = ChrootInstance::new(
            self.paths.clone(),
            app.app_id.clone(),
            instance_id,
            root,
            uid,
            gid,
            supplementary_gids,
            host_filesystem,
            host_audio,
            host_cursor,
            host_fonts,
            host_portal,
            host_graphics,
            host_video,
            app_extensions,
            pending_run.commit(),
        );

        instance.mount_nullfs(&app.runtime_dir.join("files"), "usr", true)?;
        instance.mount_nullfs(&app.app_dir.join("files"), "app", true)?;
        let app_data = self.paths.app_data(&app.app_id)?;
        for name in ["data", "config", "cache"] {
            let source = app_data.join(name);
            fs::create_dir_all(&source)
                .with_context(|| format!("create persistent app directory {}", source.display()))?;
            instance.mount_nullfs(&source, PathBuf::from("var").join(name), false)?;
        }
        for extension in instance.app_extensions.clone() {
            instance.mount_nullfs(
                &extension.checkout_dir.join("files"),
                PathBuf::from("app").join(&extension.app_mount_relative),
                true,
            )?;
        }
        for mount in instance.host_graphics.runtime_mounts() {
            instance.mount_nullfs(mount.host_path(), mount.sandbox_target_relative()?, true)?;
        }
        for mount in instance.host_video.runtime_mounts() {
            instance.mount_nullfs(mount.host_path(), mount.sandbox_target_relative()?, true)?;
        }
        for grant in instance.host_filesystem.grants().to_vec() {
            instance.mount_nullfs(
                grant.host_path(),
                grant.sandbox_target_relative()?,
                grant.access().is_read_only(),
            )?;
        }
        for mount in instance.host_cursor.mounts().to_vec() {
            instance.mount_nullfs(mount.host_path(), mount.sandbox_target_relative()?, true)?;
        }
        for mount in instance.host_fonts.mounts().to_vec() {
            instance.mount_nullfs(mount.host_path(), mount.sandbox_target_relative()?, true)?;
        }
        instance.mount_nullfs(&desktop.xdg_runtime_dir, format!("run/user/{uid}"), false)?;
        if let Some(doc_dir) = instance.host_portal.doc_dir().map(Path::to_path_buf) {
            instance.mount_nullfs(&doc_dir, format!("run/user/{uid}/doc"), true)?;
            instance.host_portal.attach_sandbox()?;
        }
        instance.mount_nullfs(Path::new("/tmp"), "tmp", false)?;
        instance.mount_special("dev", "devfs", "devfs")?;
        instance.mount_special("dev/fd", "fdescfs", "fdescfs")?;
        instance.mount_tmpfs("dev/shm", "mode=1777")?;
        instance.mount_special("proc", "linprocfs", "linprocfs")?;
        instance.mount_special("sys", "linsysfs", "linsysfs")?;
        for mount in instance.host_graphics.sysfs_mounts() {
            instance.mount_nullfs(mount.host_path(), mount.sandbox_target_relative()?, true)?;
        }

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
    paths: Installation,
    app_id: String,
    instance_id: String,
    root: PathBuf,
    uid: u32,
    gid: u32,
    supplementary_gids: Vec<u32>,
    host_filesystem: HostFilesystem,
    host_audio: HostAudio,
    host_cursor: HostCursorTheme,
    host_fonts: HostFonts,
    host_portal: HostPortal,
    host_graphics: HostGraphics,
    host_video: HostVideo,
    app_extensions: Vec<runtime::AppExtension>,
    owned_mounts: Vec<OwnedMount>,
    run_record: PathBuf,
    cleaned: bool,
}

#[derive(Debug)]
struct OwnedMount {
    path: PathBuf,
    read_only: bool,
}

impl ChrootInstance {
    #[allow(clippy::too_many_arguments)]
    fn new(
        paths: Installation,
        app_id: String,
        instance_id: String,
        root: PathBuf,
        uid: u32,
        gid: u32,
        supplementary_gids: Vec<u32>,
        host_filesystem: HostFilesystem,
        host_audio: HostAudio,
        host_cursor: HostCursorTheme,
        host_fonts: HostFonts,
        host_portal: HostPortal,
        host_graphics: HostGraphics,
        host_video: HostVideo,
        app_extensions: Vec<runtime::AppExtension>,
        run_record: PathBuf,
    ) -> Self {
        Self {
            paths,
            app_id,
            instance_id,
            root,
            uid,
            gid,
            supplementary_gids,
            host_filesystem,
            host_audio,
            host_cursor,
            host_fonts,
            host_portal,
            host_graphics,
            host_video,
            app_extensions,
            owned_mounts: Vec::new(),
            run_record,
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

    fn mount_tmpfs(&mut self, target_relative: impl AsRef<Path>, options: &str) -> Result<()> {
        let target = self.root.join(target_relative);

        let mut mkdir = Command::new("doas");
        mkdir.arg("mkdir").arg("-p").arg(&target);
        run_command(
            mkdir,
            &format!("create tmpfs mount target {}", target.display()),
        )?;
        ensure_mountpoint_free(&target)?;

        let mut command = Command::new("doas");
        command
            .arg("mount")
            .arg("-t")
            .arg("tmpfs")
            .arg("-o")
            .arg(options)
            .arg("tmpfs")
            .arg(&target);
        run_command(command, &format!("mount tmpfs {}", target.display()))?;
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
        env.extend(self.host_cursor.env());
        env.extend(self.host_portal.env());
        let metadata_env = app_metadata_env(app, &env)?;
        merge_env(&mut env, metadata_env);
        merge_env(&mut env, self.host_graphics.env());
        apply_graphics_preloads(
            &mut env,
            self.host_graphics.ld_preload_paths(),
            self.host_graphics.zypak_ld_preload_paths(),
        );
        prepend_env_paths(
            &mut env,
            "LD_LIBRARY_PATH",
            self.host_video.ld_library_paths(),
        );
        prepend_env_paths(
            &mut env,
            "LD_LIBRARY_PATH",
            app_extension_ld_paths(&self.app_extensions),
        );
        merge_env(&mut env, self.host_video.env());
        ensure_metadata_runtime_dirs(&env, &desktop.xdg_runtime_dir, self.uid, &app.app_id)?;
        let translated_args = self.host_filesystem.translate_args(&app.args)?;
        let app_args = launch_args(app, translated_args)?;

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
        for line in self.host_cursor.describe() {
            eprintln!("  cursor: {line}");
        }
        for warning in self.host_cursor.warnings() {
            eprintln!("  cursor warning: {warning}");
        }
        for line in self.host_fonts.describe() {
            eprintln!("  fonts: {line}");
        }
        for warning in self.host_fonts.warnings() {
            eprintln!("  fonts warning: {warning}");
        }
        for line in self.host_portal.describe() {
            eprintln!("  portal: {line}");
        }
        for warning in self.host_portal.warnings() {
            eprintln!("  portal warning: {warning}");
        }
        for line in self.host_graphics.describe() {
            eprintln!("  graphics: {line}");
        }
        for warning in self.host_graphics.warnings() {
            eprintln!("  graphics warning: {warning}");
        }
        for line in self.host_video.describe() {
            eprintln!("  video: {line}");
        }
        for warning in self.host_video.warnings() {
            eprintln!("  video warning: {warning}");
        }
        for extension in &self.app_extensions {
            eprintln!(
                "  app extension: {} ({}) -> /app/{}",
                extension.name,
                extension.ref_name,
                extension.app_mount_relative.display()
            );
        }
        eprintln!("  entry: {}", entry.display(&app_args));

        let mut command = Command::new("doas");
        command
            .arg("chroot")
            .arg("-u")
            .arg(self.uid.to_string())
            .arg("-g")
            .arg(self.gid.to_string());
        if !self.supplementary_gids.is_empty() {
            command
                .arg("-G")
                .arg(join_numeric_ids(&self.supplementary_gids));
        }
        command.arg(&self.root).arg("/usr/bin/env").arg("-i");
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
        state::write_run_record(
            &self.paths,
            &self.app_id,
            &self.instance_id,
            &self.root,
            std::process::id(),
            child.id(),
        )?;
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
        if let Err(error) = terminate_chroot_processes(&self.root) {
            errors.push(format!("{error:#}"));
        }
        if let Err(error) = self.host_portal.cleanup() {
            errors.push(format!("{error:#}"));
        }
        for mount in self.owned_mounts.iter().rev() {
            if let Err(error) =
                unmount_mountpoint(&mount.path, mount.read_only, "umount owned mount")
            {
                errors.push(format!("{error:#}"));
            }
        }
        if let Err(error) = self.host_graphics.cleanup() {
            errors.push(format!("{error:#}"));
        }
        if let Err(error) = self.host_audio.cleanup(&self.root) {
            errors.push(format!("{error:#}"));
        }

        if errors.is_empty() {
            remove_instance_root(&self.root)?;
            state::remove_run_record(&self.run_record)?;
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

fn prepare_root(root: &Path, uid: u32, runtime_etc: &Path, network_enabled: bool) -> Result<()> {
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
    prepare_etc_overlay(root, runtime_etc, network_enabled)?;
    Ok(())
}

fn prepare_etc_overlay(root: &Path, runtime_etc: &Path, network_enabled: bool) -> Result<()> {
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
        if matches!(name_text, "resolv.conf" | "hosts") {
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

fn app_allows_network(metadata_path: &Path) -> Result<bool> {
    let metadata = fs::read_to_string(metadata_path)
        .with_context(|| format!("read {}", metadata_path.display()))?;
    Ok(runtime::metadata_value(&metadata, "Context", "shared")
        .map(|shared| {
            shared
                .split(';')
                .map(str::trim)
                .any(|permission| permission == "network")
        })
        .unwrap_or(false))
}

fn write_flatpak_info(root: &Path, app: &FlatpakApp, instance_id: &str) -> Result<()> {
    let data = format!(
        "\
[Application]
name={}
runtime={}

[Instance]
instance-id={}

[Context]
filesystems=
",
        app.app_id, app.runtime_ref, instance_id
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
        ("XDG_SESSION_TYPE".to_string(), "wayland".to_string()),
        ("LANG".to_string(), "C.UTF-8".to_string()),
        ("LC_ALL".to_string(), "C.UTF-8".to_string()),
        ("GDK_BACKEND".to_string(), "wayland".to_string()),
        ("GSK_RENDERER".to_string(), "cairo".to_string()),
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
    push_host_env(&mut env, "XDG_CURRENT_DESKTOP");
    push_host_env(&mut env, "XDG_SESSION_DESKTOP");
    push_host_env(&mut env, "MOZ_ENABLE_WAYLAND");
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

fn app_metadata_env(
    app: &FlatpakApp,
    base_env: &[(String, String)],
) -> Result<Vec<(String, String)>> {
    let metadata_path = app.app_dir.join("metadata");
    let metadata = fs::read_to_string(&metadata_path)
        .with_context(|| format!("read Flatpak metadata {}", metadata_path.display()))?;
    let mut env = base_env.to_vec();
    let mut updates = Vec::new();
    for (key, raw_value) in runtime::metadata_section_entries(&metadata, "Environment") {
        let value = expand_env_value(&raw_value, &env);
        if let Some((_, existing)) = env
            .iter_mut()
            .find(|(existing_key, _)| existing_key == &key)
        {
            *existing = value.clone();
        } else {
            env.push((key.clone(), value.clone()));
        }
        updates.push((key, value));
    }
    Ok(updates)
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

fn ensure_metadata_runtime_dirs(
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

fn merge_env(env: &mut Vec<(String, String)>, updates: Vec<(String, String)>) {
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

fn prepend_env_paths(env: &mut Vec<(String, String)>, key: &str, paths: Vec<String>) {
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

fn apply_graphics_preloads(
    env: &mut Vec<(String, String)>,
    ld_preload_paths: Vec<String>,
    zypak_ld_preload_paths: Vec<String>,
) {
    prepend_env_paths(env, "LD_PRELOAD", ld_preload_paths);
    prepend_env_paths(env, "ZYPAK_LD_PRELOAD", zypak_ld_preload_paths);
}

fn app_extension_ld_paths(extensions: &[runtime::AppExtension]) -> Vec<String> {
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

fn launch_args(app: &FlatpakApp, translated_args: Vec<String>) -> Result<Vec<String>> {
    let mut args = compatibility_args(app, &translated_args)?;
    args.extend(translated_args);
    Ok(args)
}

fn compatibility_args(app: &FlatpakApp, translated_args: &[String]) -> Result<Vec<String>> {
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
    runtime::metadata_value(metadata, "Application", "base")
        .is_some_and(|base| base.starts_with("app/org.electronjs.Electron2.BaseApp/"))
}

fn app_requests_socket(metadata: &str, socket: &str) -> bool {
    runtime::metadata_value(metadata, "Context", "sockets")
        .map(|sockets| sockets.split(';').any(|entry| entry == socket))
        .unwrap_or(false)
}

fn has_ozone_platform_arg(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == "--ozone-platform" || arg.starts_with("--ozone-platform="))
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

fn numeric_ids(program: &str, arg: &str) -> Result<Vec<u32>> {
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

fn join_numeric_ids(ids: &[u32]) -> String {
    ids.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
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

fn terminate_chroot_mount_holders(root: &Path) -> Result<()> {
    let mut pids = BTreeSet::new();
    for mountpoint in mount_points_under(root)? {
        for pid in mount_holders(&mountpoint)? {
            if pid == std::process::id() as i32 {
                continue;
            }
            if process_rooted_in(pid, root)? {
                pids.insert(pid);
            }
        }
    }

    for pid in pids {
        eprintln!("recovering stale sandbox mount holder pid {pid}");
        terminate_process(pid);
    }
    Ok(())
}

fn terminate_chroot_processes(root: &Path) -> Result<()> {
    let output = Command::new("ps")
        .args(["-axo", "pid"])
        .output()
        .context("list processes for sandbox cleanup")?;
    if !output.status.success() {
        bail!("ps -axo pid failed with status {}", output.status);
    }
    let text = String::from_utf8(output.stdout)?;
    let mut pids = BTreeSet::new();
    for line in text.lines().skip(1) {
        let Ok(pid) = line.trim().parse::<i32>() else {
            continue;
        };
        if pid == std::process::id() as i32 {
            continue;
        }
        if process_rooted_in(pid, root)? {
            pids.insert(pid);
        }
    }

    for pid in pids {
        eprintln!("terminating remaining sandbox process pid {pid}");
        terminate_process(pid);
    }
    Ok(())
}

fn mount_holders(mountpoint: &Path) -> Result<Vec<i32>> {
    let output = Command::new("fstat")
        .arg("-f")
        .arg(mountpoint)
        .output()
        .with_context(|| format!("find mount holders for {}", mountpoint.display()))?;
    if !output.status.success() {
        bail!(
            "fstat -f {} failed with status {}",
            mountpoint.display(),
            output.status
        );
    }
    let text = String::from_utf8(output.stdout)?;
    Ok(text
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().nth(2))
        .filter_map(|pid| pid.parse::<i32>().ok())
        .collect())
}

fn process_rooted_in(pid: i32, root: &Path) -> Result<bool> {
    let output = Command::new("procstat")
        .arg("-f")
        .arg(pid.to_string())
        .output()
        .with_context(|| format!("inspect process {pid} root"))?;
    if !output.status.success() {
        return Ok(false);
    }
    let text = String::from_utf8(output.stdout)?;
    Ok(text.lines().any(|line| {
        let mut fields = line.split_whitespace();
        let _pid = fields.next();
        let _comm = fields.next();
        let Some(fd) = fields.next() else {
            return false;
        };
        if fd != "root" && fd != "jail" && fd != "cwd" {
            return false;
        }
        fields.last().is_some_and(|path| Path::new(path) == root)
    }))
}

fn chroot_root_for_mount(chroot_root: &Path, mountpoint: &Path) -> Option<PathBuf> {
    let mut candidate = mountpoint.parent();
    while let Some(path) = candidate {
        if path == chroot_root {
            break;
        }
        if path.starts_with(chroot_root) && path.join(".flatpak-info").is_file() {
            return Some(path.to_path_buf());
        }
        candidate = path.parent();
    }
    None
}

fn remove_instance_root(root: &Path) -> Result<()> {
    match fs::remove_dir_all(root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove sandbox root {}", root.display())),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_ID: AtomicUsize = AtomicUsize::new(0);

    fn test_dir(name: &str) -> PathBuf {
        let id = TEST_ID.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "freebsd-flatpak-poc-sandbox-{name}-{}-{id}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn app_with_metadata(metadata: &str) -> FlatpakApp {
        let app_dir = test_dir("compat-app");
        fs::write(app_dir.join("metadata"), metadata).unwrap();
        FlatpakApp {
            app_id: "org.example.App".to_string(),
            app_dir,
            runtime_ref: "org.freedesktop.Platform/x86_64/25.08".to_string(),
            runtime_dir: PathBuf::from("/runtime"),
            command: "app".to_string(),
            args: Vec::new(),
        }
    }

    #[test]
    fn concurrent_instances_get_distinct_roots_and_cleanup_isolation() {
        let dir = test_dir("concurrent-instance-roots");
        let app_root = dir.join("chroots").join("org.example.App");
        let first_id = new_instance_id();
        let second_id = new_instance_id();
        let first_root = app_root.join(&first_id);
        let second_root = app_root.join(&second_id);
        fs::create_dir_all(first_root.join("usr")).unwrap();
        fs::create_dir_all(second_root.join("usr")).unwrap();
        fs::write(first_root.join(".flatpak-info"), "first").unwrap();
        fs::write(second_root.join(".flatpak-info"), "second").unwrap();

        assert_ne!(first_id, second_id);
        assert_eq!(
            chroot_root_for_mount(&dir.join("chroots"), &first_root.join("usr")),
            Some(first_root.clone())
        );
        assert_eq!(
            chroot_root_for_mount(&dir.join("chroots"), &second_root.join("usr")),
            Some(second_root.clone())
        );

        remove_instance_root(&first_root).unwrap();
        assert!(!first_root.exists());
        assert!(second_root.join(".flatpak-info").is_file());
    }

    #[test]
    fn electron_base_app_gets_wayland_ozone_arg() {
        let app = app_with_metadata(
            "[Application]\nbase=app/org.electronjs.Electron2.BaseApp/x86_64/25.08\n\n[Context]\nsockets=wayland;x11;\n",
        );

        assert_eq!(
            compatibility_args(&app, &[]).unwrap(),
            vec!["--ozone-platform=wayland"]
        );
    }

    #[test]
    fn explicit_ozone_arg_is_preserved() {
        let app = app_with_metadata(
            "[Application]\nbase=app/org.electronjs.Electron2.BaseApp/x86_64/25.08\n\n[Context]\nsockets=wayland;x11;\n",
        );

        assert!(
            compatibility_args(&app, &["--ozone-platform=x11".to_string()])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn non_electron_app_gets_no_ozone_arg() {
        let app = app_with_metadata(
            "[Application]\nbase=app/org.gnome.Platform/x86_64/49\n\n[Context]\nsockets=wayland;\n",
        );

        assert!(compatibility_args(&app, &[]).unwrap().is_empty());
    }

    #[test]
    fn app_metadata_environment_expands_existing_sandbox_values() {
        let app = app_with_metadata(
            "[Environment]\nMESA_SHADER_CACHE_DIR=$XDG_RUNTIME_DIR/app/$FLATPAK_ID/cache/mesa_shader_cache_db\nGTK_PATH=/app/lib/gtkmodules\n",
        );
        let base_env = vec![
            ("XDG_RUNTIME_DIR".to_string(), "/run/user/1001".to_string()),
            ("FLATPAK_ID".to_string(), "org.example.App".to_string()),
        ];

        assert_eq!(
            app_metadata_env(&app, &base_env).unwrap(),
            vec![
                (
                    "MESA_SHADER_CACHE_DIR".to_string(),
                    "/run/user/1001/app/org.example.App/cache/mesa_shader_cache_db".to_string()
                ),
                ("GTK_PATH".to_string(), "/app/lib/gtkmodules".to_string())
            ]
        );
    }

    #[test]
    fn app_metadata_environment_supports_braced_variables() {
        let app =
            app_with_metadata("[Environment]\nEXAMPLE=${XDG_RUNTIME_DIR}/app/${FLATPAK_ID}\n");
        let base_env = vec![
            ("XDG_RUNTIME_DIR".to_string(), "/run/user/1001".to_string()),
            ("FLATPAK_ID".to_string(), "org.example.App".to_string()),
        ];

        assert_eq!(
            app_metadata_env(&app, &base_env).unwrap(),
            vec![(
                "EXAMPLE".to_string(),
                "/run/user/1001/app/org.example.App".to_string()
            )]
        );
    }

    #[test]
    fn ordinary_apps_keep_graphics_shims_in_ld_preload() {
        let mut env = Vec::new();

        apply_graphics_preloads(
            &mut env,
            vec!["/shim/drm.so".to_string(), "/shim/wayland.so".to_string()],
            vec!["/shim/wayland.so".to_string(), "/shim/drm.so".to_string()],
        );

        assert_eq!(
            env.iter()
                .find(|(key, _)| key == "LD_PRELOAD")
                .map(|(_, value)| value.as_str()),
            Some("/shim/drm.so:/shim/wayland.so")
        );
    }

    #[test]
    fn zypak_gets_both_graphics_shims() {
        let mut env = Vec::new();

        apply_graphics_preloads(
            &mut env,
            vec!["/shim/drm.so".to_string(), "/shim/wayland.so".to_string()],
            vec!["/shim/wayland.so".to_string(), "/shim/drm.so".to_string()],
        );

        assert_eq!(
            env.iter()
                .find(|(key, _)| key == "ZYPAK_LD_PRELOAD")
                .map(|(_, value)| value.as_str()),
            Some("/shim/wayland.so:/shim/drm.so")
        );
    }

    #[test]
    fn zypak_graphics_preloads_preserve_existing_value() {
        let mut env = vec![(
            "ZYPAK_LD_PRELOAD".to_string(),
            "/app/existing.so".to_string(),
        )];

        apply_graphics_preloads(
            &mut env,
            Vec::new(),
            vec!["/shim/wayland.so".to_string(), "/shim/drm.so".to_string()],
        );

        assert_eq!(
            env.iter()
                .find(|(key, _)| key == "ZYPAK_LD_PRELOAD")
                .map(|(_, value)| value.as_str()),
            Some("/shim/wayland.so:/shim/drm.so:/app/existing.so")
        );
    }

    #[test]
    fn normal_and_zypak_preload_values_remain_independent() {
        let mut env = vec![
            ("LD_PRELOAD".to_string(), "/app/normal.so".to_string()),
            ("ZYPAK_LD_PRELOAD".to_string(), "/app/zypak.so".to_string()),
        ];

        apply_graphics_preloads(
            &mut env,
            vec!["/shim/drm.so".to_string()],
            vec!["/shim/wayland.so".to_string()],
        );

        assert_eq!(
            env.iter()
                .find(|(key, _)| key == "LD_PRELOAD")
                .map(|(_, value)| value.as_str()),
            Some("/shim/drm.so:/app/normal.so")
        );
        assert_eq!(
            env.iter()
                .find(|(key, _)| key == "ZYPAK_LD_PRELOAD")
                .map(|(_, value)| value.as_str()),
            Some("/shim/wayland.so:/app/zypak.so")
        );
    }

    #[test]
    fn metadata_runtime_dirs_are_created_inside_app_runtime_scope() {
        let dir = test_dir("metadata-runtime-dirs");
        let host_runtime = dir.join("xdg-runtime");
        fs::create_dir_all(&host_runtime).unwrap();
        let env = vec![
            (
                "MESA_SHADER_CACHE_DIR".to_string(),
                "/run/user/1001/app/org.example.App/cache/mesa_shader_cache_db".to_string(),
            ),
            (
                "OTHER_DIR".to_string(),
                "/run/user/1001/other/cache".to_string(),
            ),
        ];

        ensure_metadata_runtime_dirs(&env, &host_runtime, 1001, "org.example.App").unwrap();

        assert!(host_runtime
            .join("app/org.example.App/cache/mesa_shader_cache_db")
            .is_dir());
        assert!(!host_runtime.join("other/cache").exists());
    }

    #[test]
    fn shared_network_enables_resolver_overlay() {
        let dir = test_dir("network-metadata");
        let metadata = dir.join("metadata");
        fs::write(&metadata, "[Context]\nshared=ipc;network;\n").unwrap();

        assert!(app_allows_network(&metadata).unwrap());
    }

    #[test]
    fn missing_network_shared_permission_disables_resolver_overlay() {
        let dir = test_dir("non-network-metadata");
        let metadata = dir.join("metadata");
        fs::write(&metadata, "[Context]\nshared=ipc;\n").unwrap();

        assert!(!app_allows_network(&metadata).unwrap());
    }

    #[test]
    fn etc_overlay_preserves_runtime_etc_and_adds_network_resolver_files() {
        let dir = test_dir("etc-overlay");
        let root = dir.join("root");
        let runtime_etc = dir.join("runtime-etc");
        fs::create_dir_all(&runtime_etc).unwrap();
        fs::write(runtime_etc.join("nsswitch.conf"), "hosts: files dns\n").unwrap();

        prepare_etc_overlay(&root, &runtime_etc, true).unwrap();

        assert!(root.join("etc/resolv.conf").is_file());
        assert!(root.join("etc/hosts").is_file());
        assert_eq!(
            fs::read_link(root.join("etc/nsswitch.conf")).unwrap(),
            PathBuf::from("/usr/etc/nsswitch.conf")
        );
    }
}
