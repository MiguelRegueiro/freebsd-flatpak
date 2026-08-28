use super::application_entrypoint::{host_user, join_numeric_ids, launch_args, EntryLaunch};
use super::filesystem_grants::HostFilesystem;
use super::launch_application::FlatpakApp;
use super::launch_environment::{
    app_extension_ld_paths, app_metadata_env, apply_graphics_preloads, apply_unset_environment,
    ensure_metadata_runtime_dirs, launch_env, merge_env, prepend_env_paths,
};
use super::mount_operations::owned_mount_teardown_order;
use super::process_signals::{install_signal_handlers, ACTIVE_PROCESS_GROUP, LAST_SIGNAL};
use super::process_supervision::wait_for_sandbox_processes;
use super::stale_sandbox_recovery::{
    remove_instance_root, terminate_chroot_processes, unmount_mountpoint,
};
use crate::desktop_integration::DesktopSession;
use crate::diagnostics::{Detail, Diagnostics};
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
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::Ordering;

#[derive(Debug)]
pub(super) struct ChrootInstance {
    pub(super) paths: Installation,
    instance_id: String,
    pub(super) root: PathBuf,
    uid: u32,
    gid: u32,
    supplementary_gids: Vec<u32>,
    pub(super) host_filesystem: HostFilesystem,
    effective_metadata: String,
    host_audio: HostAudio,
    pub(super) host_cursor: HostCursorTheme,
    pub(super) host_fonts: HostFonts,
    pub(super) host_portal: HostPortal,
    pub(super) host_graphics: HostGraphics,
    pub(super) host_network: HostNetwork,
    pub(super) host_video: HostVideo,
    pub(super) app_extensions: Vec<runtime::AppExtension>,
    pub(super) owned_mounts: Vec<OwnedMount>,
    run_record: PathBuf,
    deployment: state::AppRecord,
    extension_refs: Vec<String>,
    cleaned: bool,
    pub(super) nullfs_mounts: Vec<NullfsMapping>,
    pub(super) mount_staging_ready: bool,
    pub(super) next_mount_staging_id: usize,
}

#[derive(Debug, Clone)]
pub(super) struct OwnedMount {
    pub(super) path: PathBuf,
    pub(super) read_only: bool,
}

#[derive(Debug, Clone)]
pub(super) struct NullfsMapping {
    pub(super) source: PathBuf,
    pub(super) target: PathBuf,
}

impl ChrootInstance {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        paths: Installation,
        instance_id: String,
        root: PathBuf,
        uid: u32,
        gid: u32,
        supplementary_gids: Vec<u32>,
        host_filesystem: HostFilesystem,
        effective_metadata: String,
        host_audio: HostAudio,
        host_cursor: HostCursorTheme,
        host_fonts: HostFonts,
        host_portal: HostPortal,
        host_graphics: HostGraphics,
        host_network: HostNetwork,
        host_video: HostVideo,
        app_extensions: Vec<runtime::AppExtension>,
        run_record: PathBuf,
        deployment: state::AppRecord,
        extension_refs: Vec<String>,
    ) -> Self {
        Self {
            paths,
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
            host_network,
            host_video,
            app_extensions,
            owned_mounts: Vec::new(),
            run_record,
            deployment,
            extension_refs,
            cleaned: false,
            nullfs_mounts: Vec::new(),
            mount_staging_ready: false,
            next_mount_staging_id: 0,
        }
    }

    pub(super) fn launch(
        &mut self,
        app: &FlatpakApp,
        desktop: &DesktopSession,
        entry: &EntryLaunch,
        diagnostics: &Diagnostics,
    ) -> Result<ExitStatus> {
        let launch = diagnostics.timer(Detail::Summary);
        let launch_configuration = diagnostics.timer(Detail::Detailed);
        install_signal_handlers();
        let user = host_user(self.uid);
        let mut env = launch_env(app, desktop, self.uid, &user, self.paths.data_home());
        env.retain(|(key, _)| key != "HOME");
        env.push((
            "HOME".to_string(),
            self.host_filesystem.sandbox_home_env("/var/data"),
        ));
        env.extend(self.host_filesystem.user_dir_env());
        env.extend(self.host_audio.env());
        env.extend(self.host_cursor.env());
        prepend_env_paths(&mut env, "XDG_CONFIG_DIRS", self.host_cursor.config_dirs());
        env.extend(self.host_portal.env());
        let metadata_env = app_metadata_env(&self.effective_metadata, &env);
        merge_env(&mut env, metadata_env);
        apply_unset_environment(&mut env, &self.effective_metadata);
        merge_env(&mut env, self.host_graphics.env());
        apply_graphics_preloads(
            &mut env,
            self.host_graphics.ld_preload_paths(),
            self.host_graphics.zypak_ld_preload_paths(),
        );
        prepend_env_paths(&mut env, "LD_PRELOAD", self.host_network.preload_paths());
        prepend_env_paths(
            &mut env,
            "ZYPAK_LD_PRELOAD",
            self.host_network.preload_paths(),
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
        launch_configuration.finish("launch", "environment and arguments");

        diagnostics.message(Detail::Detailed, || {
            format!("launching {} from {}", app.app_id, self.root.display())
        });
        if diagnostics.enabled(Detail::Detailed) {
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
                eprintln!("  desktop theme: {line}");
            }
            for warning in self.host_cursor.warnings() {
                eprintln!("  desktop theme warning: {warning}");
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
        }

        let spawn = diagnostics.timer(Detail::Detailed);
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
            .stderr(Stdio::inherit())
            .process_group(0);

        LAST_SIGNAL.store(0, Ordering::SeqCst);
        let mut child = command.spawn().context("launch app through chroot")?;
        spawn.finish("launch", "build command and spawn");
        launch.finish("run", "application spawn");
        diagnostics.startup_complete();
        // Launchers may exit after backgrounding the real application. Its
        // processes inherit this group, so keep the sandbox until they exit.
        let process_group = child.id() as i32;
        ACTIVE_PROCESS_GROUP.store(process_group, Ordering::SeqCst);
        let launch_result: Result<ExitStatus> = (|| {
            state::write_pinned_run_record_with_extensions(
                &self.paths,
                &self.instance_id,
                &self.root,
                std::process::id(),
                child.id(),
                &self.deployment,
                &self.extension_refs,
            )?;
            let status = child.wait().context("wait for app process")?;
            wait_for_sandbox_processes(&self.root, process_group, || {
                matches!(
                    LAST_SIGNAL.load(Ordering::SeqCst),
                    libc::SIGINT | libc::SIGTERM
                )
            })?;
            Ok(status)
        })();
        ACTIVE_PROCESS_GROUP.store(0, Ordering::SeqCst);
        let status = launch_result?;

        let signal = LAST_SIGNAL.swap(0, Ordering::SeqCst);
        if signal != 0 {
            eprintln!("received signal {signal}; app process exited, cleaning up sandbox");
        }

        Ok(status)
    }

    pub(super) fn cleanup(&mut self) -> Result<()> {
        let mut errors = Vec::new();
        if let Err(error) = terminate_chroot_processes(&self.root) {
            errors.push(format!("{error:#}"));
        }
        if let Err(error) = self.host_portal.cleanup() {
            errors.push(format!("{error:#}"));
        }
        let mut remaining_mounts = Vec::new();
        let owned_mounts = std::mem::take(&mut self.owned_mounts);
        match owned_mount_teardown_order(&self.root, owned_mounts.clone()) {
            Ok(mounts) => {
                for mount in mounts {
                    if let Err(error) =
                        unmount_mountpoint(&mount.path, mount.read_only, "umount owned mount")
                    {
                        errors.push(format!("{error:#}"));
                        remaining_mounts.push(mount);
                    }
                }
            }
            Err(error) => {
                errors.push(format!("{error:#}"));
                remaining_mounts = owned_mounts;
            }
        }
        self.owned_mounts = remaining_mounts;
        // The generated graphics trees are nullfs sources. Keep them until
        // every owned target has detached, so a busy mount remains recoverable
        // on a later cleanup pass.
        if self.owned_mounts.is_empty() {
            if let Err(error) = self.host_graphics.cleanup() {
                errors.push(format!("{error:#}"));
            }
        }
        if let Err(error) = self.host_audio.cleanup(&self.root) {
            errors.push(format!("{error:#}"));
        }

        if errors.is_empty() {
            remove_instance_root(&self.root)?;
            state::remove_run_record(&self.run_record)?;
            state::cleanup_retired_deployments(&self.paths)?;
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
