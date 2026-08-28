use super::application_entrypoint::{numeric_id, numeric_ids, resolve_entry, sandbox_name};
use super::chroot_instance::ChrootInstance;
use super::filesystem_grants::HostFilesystem;
use super::flatpak_data_mount_plan::FlatpakDataMountPlan;
use super::flatpak_installation::FlatpakInstallationProjection;
use super::launch_application::FlatpakApp;
use super::process_signals::install_signal_handlers;
use super::sandbox_identity::SandboxIdentity;
use super::sandbox_root::{app_allows_network, prepare_root, write_flatpak_info};
use super::static_overrides::{effective_metadata, permission_enabled};
use crate::desktop_integration::DesktopSession;
use crate::diagnostics::{Detail, Diagnostics};
use crate::host_resources::audio::HostAudio;
use crate::host_resources::cursor_themes::HostCursorTheme;
use crate::host_resources::fonts::HostFonts;
use crate::host_resources::graphics::HostGraphics;
use crate::host_resources::linux_compat::HostLinuxCompat;
use crate::host_resources::network::HostNetwork;
use crate::host_resources::system_bus::HostSystemBus;
use crate::host_resources::video_acceleration::HostVideo;
use crate::installation as runtime;
use crate::installation as state;
use crate::installation::installation_paths::Installation;
use crate::portal_integration::HostPortal;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static INSTANCE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub trait SandboxBackend {
    fn run(
        &self,
        app: &FlatpakApp,
        desktop: &DesktopSession,
        diagnostics: &Diagnostics,
    ) -> Result<ExitStatus>;
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
    deployment: state::AppRecord,
    instance_id: String,
    root: PathBuf,
    extension_refs: Vec<String>,
    committed: bool,
}

impl PendingRunRecord {
    fn new(paths: &Installation, app: &FlatpakApp, instance_id: &str, root: &Path) -> Result<Self> {
        let deployment =
            state::pinned_deployment_for_app(paths, &app.app_id, &app.app_dir, &app.runtime_dir)?;
        let path = state::write_pinned_run_record(
            paths,
            instance_id,
            root,
            std::process::id(),
            0,
            &deployment,
        )?;
        Ok(Self {
            path,
            deployment,
            instance_id: instance_id.to_string(),
            root: root.to_path_buf(),
            extension_refs: Vec::new(),
            committed: false,
        })
    }

    fn set_extensions(&mut self, paths: &Installation, extension_refs: Vec<String>) -> Result<()> {
        state::write_pinned_run_record_with_extensions(
            paths,
            &self.instance_id,
            &self.root,
            std::process::id(),
            0,
            &self.deployment,
            &extension_refs,
        )?;
        self.extension_refs = extension_refs;
        Ok(())
    }

    fn commit(mut self) -> (PathBuf, state::AppRecord, Vec<String>) {
        self.committed = true;
        (
            self.path.clone(),
            self.deployment.clone(),
            self.extension_refs.clone(),
        )
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

    fn prepare(
        &self,
        app: &FlatpakApp,
        desktop: &DesktopSession,
        diagnostics: &Diagnostics,
    ) -> Result<ChrootInstance> {
        let host_resources = diagnostics.timer(Detail::Summary);
        let identity_timing = diagnostics.timer(Detail::Detailed);
        let uid = numeric_id("id", "-u")?;
        let gid = numeric_id("id", "-g")?;
        let supplementary_gids = numeric_ids("id", "-G")?;
        let identity = SandboxIdentity::from_current_process(uid, gid)?;
        let user = identity.user_name().to_string();
        let instance_id = new_instance_id();
        let root = self
            .paths
            .chroots()
            .join(sandbox_name(&app.app_id))
            .join(&instance_id);
        let mut pending_run = PendingRunRecord::new(&self.paths, app, &instance_id, &root)?;
        identity_timing.finish("sandbox", "identity and instance paths");
        let metadata = diagnostics.timer(Detail::Detailed);
        let metadata_path = app.app_dir.join("metadata");
        let effective_metadata =
            effective_metadata(&metadata_path, &self.paths.flatpak_overrides(), &app.app_id)?;
        let expose_flatpak_apps = permission_enabled(
            &effective_metadata,
            "Context",
            "filesystems",
            "xdg-data/flatpak/app",
        );
        let network_enabled = app_allows_network(&effective_metadata);
        let host_linux_compat = HostLinuxCompat::prepare(&self.paths)?;
        let host_network = HostNetwork::prepare(&self.paths, network_enabled)?;
        let host_system_bus =
            HostSystemBus::prepare(&self.paths, &effective_metadata, &instance_id)?;
        let host_filesystem = HostFilesystem::from_metadata_for_user(
            &effective_metadata,
            &user,
            self.paths.data_root(),
            &root,
        )?;
        let host_audio =
            HostAudio::from_metadata(&effective_metadata, &desktop.xdg_runtime_dir, uid);
        let host_cursor = HostCursorTheme::from_host(desktop);
        let host_fonts = HostFonts::from_host();
        metadata.finish("sandbox", "metadata and host resources");
        host_resources.finish("run", "host resource discovery");

        let host_portal = diagnostics.measure(Detail::Summary, "run", "portal setup", || {
            HostPortal::prepare(
                &self.paths,
                app,
                &effective_metadata,
                &instance_id,
                desktop,
                uid,
                &root,
                diagnostics,
            )
        })?;
        let graphics = diagnostics.timer(Detail::Summary);
        let host_graphics = HostGraphics::prepare(&self.paths, app, &instance_id, diagnostics)?;
        let host_video = diagnostics.measure(
            Detail::Detailed,
            "graphics",
            "activate local VAAPI extension",
            || HostVideo::prepare(&self.paths, app),
        )?;
        let app_extensions = runtime::activate_app_codec_extensions(&self.paths, app)?;
        let mut extension_refs = host_graphics
            .extension_refs()
            .chain(host_video.extension_refs())
            .map(ToOwned::to_owned)
            .chain(
                app_extensions
                    .iter()
                    .map(|extension| extension.ref_name.clone()),
            )
            .collect::<Vec<_>>();
        extension_refs.sort();
        extension_refs.dedup();
        pending_run.set_extensions(&self.paths, extension_refs)?;

        graphics.finish("run", "graphics and extensions");

        let filesystem = diagnostics.timer(Detail::Summary);
        let root_layout = diagnostics.timer(Detail::Detailed);
        prepare_root(
            &root,
            &identity,
            &app.runtime_dir.join("files").join("etc"),
            network_enabled,
        )?;
        write_flatpak_info(&root, app, &instance_id)?;
        host_audio.prepare(&root)?;
        host_cursor.prepare(&root)?;
        host_fonts.prepare(&root)?;
        let (run_record, deployment, extension_refs) = pending_run.commit();
        let mut instance = ChrootInstance::new(
            self.paths.clone(),
            instance_id,
            root,
            uid,
            gid,
            supplementary_gids,
            host_filesystem,
            effective_metadata,
            host_audio,
            host_cursor,
            host_fonts,
            host_portal,
            host_graphics,
            host_linux_compat,
            host_network,
            host_system_bus,
            host_video,
            app_extensions,
            run_record,
            deployment,
            extension_refs,
        );

        root_layout.finish("sandbox", "root layout and metadata");
        let mounts = diagnostics.timer(Detail::Detailed);

        instance.mount_nullfs(&app.runtime_dir.join("files"), "usr", true)?;
        instance.mount_nullfs(&app.app_dir.join("files"), "app", true)?;
        let app_data = self.paths.app_data(&app.app_id)?;
        for name in ["data", "config", "cache"] {
            let source = app_data.join(name);
            fs::create_dir_all(&source)
                .with_context(|| format!("create persistent app directory {}", source.display()))?;
        }
        instance
            .host_filesystem
            .write_xdg_user_dirs_config(&app_data.join("config"))?;
        for name in ["data", "config", "cache"] {
            instance.mount_nullfs_secure(
                &app_data.join(name),
                PathBuf::from("var").join(name),
                false,
            )?;
        }
        for extension in instance.app_extensions.clone() {
            instance.mount_nullfs(
                &extension.checkout_dir.join("files"),
                PathBuf::from("app").join(&extension.app_mount_relative),
                true,
            )?;
        }
        let graphics_mounts = instance.host_graphics.runtime_mounts();
        let (linux_compat_source, linux_compat_target) = instance.host_linux_compat.runtime_mount();
        let network_mount = instance.host_network.runtime_mount();
        instance.mount_nullfs(&linux_compat_source, &linux_compat_target, true)?;
        for mount in &graphics_mounts {
            let target = mount.sandbox_target_relative()?;
            if target != linux_compat_target {
                instance.mount_nullfs(mount.host_path(), target, true)?;
            }
        }
        if let Some(mount) = network_mount {
            let target = mount.sandbox_target_relative()?;
            let already_mounted = target == linux_compat_target
                || graphics_mounts
                    .iter()
                    .filter_map(|graphics| graphics.sandbox_target_relative().ok())
                    .any(|graphics_target| graphics_target == target);
            if !already_mounted {
                instance.mount_nullfs(mount.host_path(), target, true)?;
            }
        }
        if let Some((source, target)) = instance.host_system_bus.runtime_mount() {
            instance.mount_nullfs(&source, target, true)?;
        }
        for mount in instance.host_video.runtime_mounts() {
            instance.mount_nullfs(mount.host_path(), mount.sandbox_target_relative()?, true)?;
        }
        let flatpak_data_plan = FlatpakDataMountPlan::build(
            &instance.host_filesystem,
            &app_data,
            self.paths.data_home(),
            self.paths.data_root(),
        )?;
        for grant in flatpak_data_plan.grants_before_mask {
            instance.mount_nullfs_secure(
                grant.host_path(),
                grant.sandbox_target_relative()?,
                grant.access().is_read_only(),
            )?;
        }
        for root in &flatpak_data_plan.reserved_roots_to_mask {
            instance.mount_tmpfs_secure(
                absolute_to_chroot_relative(root)?,
                &format!("mode=0700,uid={uid},gid={gid}"),
            )?;
        }
        if flatpak_data_plan.mask_app_data_root {
            instance.mount_tmpfs_secure(
                absolute_to_chroot_relative(&flatpak_data_plan.app_data_root)?,
                &format!("mode=0700,uid={uid},gid={gid}"),
            )?;
        }
        for grant in flatpak_data_plan.grants_after_mask {
            instance.mount_nullfs_secure(
                grant.host_path(),
                grant.sandbox_target_relative()?,
                grant.access().is_read_only(),
            )?;
        }
        instance.mount_nullfs_secure(
            &flatpak_data_plan.app_data,
            absolute_to_chroot_relative(&flatpak_data_plan.app_data)?,
            false,
        )?;
        if expose_flatpak_apps {
            let projection = FlatpakInstallationProjection::prepare(&instance.root, &self.paths)?;
            instance.mount_nullfs_secure(&projection.source_root, &projection.target_root, true)?;
            for deployment in projection.deployments {
                instance.mount_nullfs_secure(&deployment.source, &deployment.target, true)?;
            }
        }
        for mount in instance.host_cursor.mounts().to_vec() {
            instance.mount_nullfs(mount.host_path(), mount.sandbox_target_relative()?, true)?;
        }
        for mount in instance.host_fonts.mounts().to_vec() {
            instance.mount_nullfs(mount.host_path(), mount.sandbox_target_relative()?, true)?;
        }
        instance.mount_nullfs(&desktop.xdg_runtime_dir, format!("run/user/{uid}"), false)?;
        if let Some(doc_dir) = instance.host_portal.doc_dir().map(Path::to_path_buf) {
            instance.mount_nullfs_secure(&doc_dir, format!("run/user/{uid}/doc"), true)?;
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

        mounts.finish("sandbox", "filesystem mounts");
        filesystem.finish("run", "sandbox filesystem");

        Ok(instance)
    }
}

fn absolute_to_chroot_relative(path: &Path) -> Result<PathBuf> {
    let relative = path
        .strip_prefix(Path::new("/"))
        .with_context(|| format!("make sandbox path relative: {}", path.display()))?;
    if relative.as_os_str().is_empty() {
        bail!("sandbox path must not be the root directory");
    }
    Ok(relative.to_path_buf())
}

impl SandboxBackend for ChrootNullfsBackend {
    fn run(
        &self,
        app: &FlatpakApp,
        desktop: &DesktopSession,
        diagnostics: &Diagnostics,
    ) -> Result<ExitStatus> {
        install_signal_handlers();
        if !desktop.wayland_socket().exists() {
            bail!(
                "Wayland socket does not exist: {}",
                desktop.wayland_socket().display()
            );
        }

        let entry = resolve_entry(app)?;
        let mut instance = self.prepare(app, desktop, diagnostics)?;
        let status = instance.launch(app, desktop, &entry, diagnostics)?;
        instance.cleanup()?;
        Ok(status)
    }
}

#[cfg(test)]
#[path = "tests/chroot_backend.rs"]
mod tests;
