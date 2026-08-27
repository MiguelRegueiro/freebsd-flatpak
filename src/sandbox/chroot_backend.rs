use super::application_entrypoint::{
    host_user, numeric_id, numeric_ids, resolve_entry, sandbox_name,
};
use super::chroot_instance::ChrootInstance;
use super::filesystem_grants::HostFilesystem;
use super::launch_application::FlatpakApp;
use super::process_signals::install_signal_handlers;
use super::sandbox_root::{app_allows_network, prepare_root, write_flatpak_info};
use crate::desktop_integration::DesktopSession;
use crate::host_resources::audio::HostAudio;
use crate::host_resources::cursor_themes::HostCursorTheme;
use crate::host_resources::fonts::HostFonts;
use crate::host_resources::graphics::HostGraphics;
use crate::host_resources::network::HostNetwork;
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
    fn run(&self, app: &FlatpakApp, desktop: &DesktopSession) -> Result<ExitStatus>;
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
        let mut pending_run = PendingRunRecord::new(&self.paths, app, &instance_id, &root)?;
        let metadata_path = app.app_dir.join("metadata");
        let network_enabled = app_allows_network(&metadata_path)?;
        let host_network = HostNetwork::prepare(&self.paths, network_enabled)?;
        let host_filesystem = HostFilesystem::from_metadata_file_for_user(
            &metadata_path,
            &user,
            self.paths.data_root(),
            &root,
        )?;
        let host_audio =
            HostAudio::from_metadata_file(&metadata_path, &desktop.xdg_runtime_dir, uid)?;
        let host_cursor = HostCursorTheme::from_host(desktop);
        let host_fonts = HostFonts::from_host();
        let host_portal = HostPortal::prepare(&self.paths, app, &instance_id, desktop, uid, &root)?;
        let host_graphics = HostGraphics::prepare(&self.paths, app, &instance_id)?;
        let host_video = HostVideo::prepare(&self.paths, app)?;
        let app_extensions = runtime::ensure_app_codec_extensions(&self.paths, app)?;
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

        prepare_root(
            &root,
            uid,
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
            host_audio,
            host_cursor,
            host_fonts,
            host_portal,
            host_graphics,
            host_network,
            host_video,
            app_extensions,
            run_record,
            deployment,
            extension_refs,
        );

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
            instance.mount_nullfs(&app_data.join(name), PathBuf::from("var").join(name), false)?;
        }
        for extension in instance.app_extensions.clone() {
            instance.mount_nullfs(
                &extension.checkout_dir.join("files"),
                PathBuf::from("app").join(&extension.app_mount_relative),
                true,
            )?;
        }
        let graphics_mounts = instance.host_graphics.runtime_mounts();
        let network_mount = instance.host_network.runtime_mount();
        for mount in &graphics_mounts {
            instance.mount_nullfs(mount.host_path(), mount.sandbox_target_relative()?, true)?;
        }
        if let Some(mount) = network_mount {
            let target = mount.sandbox_target_relative()?;
            let already_mounted = graphics_mounts
                .iter()
                .filter_map(|graphics| graphics.sandbox_target_relative().ok())
                .any(|graphics_target| graphics_target == target);
            if !already_mounted {
                instance.mount_nullfs(mount.host_path(), target, true)?;
            }
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

#[cfg(test)]
#[path = "tests/chroot_backend.rs"]
mod tests;
