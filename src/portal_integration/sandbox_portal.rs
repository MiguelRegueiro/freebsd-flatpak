use super::portal_scope::{
    app_scope_name, ensure_bridge_helpers, lock_portal_scope, other_active_app_instances,
    portal_control, shared_portal_dir, stop_shared_portal,
};
use super::private_session_bus::{
    detach_shared_process, private_bus_config, sandbox_bus_address, shared_portal_ready,
    start_private_bus, terminate_child, wait_for_portal_proxy,
};
use crate::desktop_integration::DesktopSession;
use crate::installation::installation_paths::Installation;
use crate::installation::FlatpakApp;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug)]
pub struct HostPortal {
    proxy: Option<PortalProxy>,
    mode: PortalMode,
    warnings: Vec<String>,
}

#[derive(Debug)]
pub(super) struct PortalProxy {
    pub(super) paths: Installation,
    pub(super) app_id: String,
    pub(super) instance_id: String,
    pub(super) shared_dir: PathBuf,
    pub(super) doc_dir: PathBuf,
    pub(super) sandbox_doc_dir: PathBuf,
    pub(super) private_bus_address: String,
    pub(super) sandbox_bus_address: String,
}

#[derive(Debug, Clone, Copy)]
enum PortalMode {
    PrivateProxy,
    Disabled,
}

impl HostPortal {
    pub fn prepare(
        paths: &Installation,
        app: &FlatpakApp,
        instance_id: &str,
        desktop: &DesktopSession,
        uid: u32,
        sandbox_root: &Path,
    ) -> Result<Self> {
        let app_id = &app.app_id;
        let mut warnings = Vec::new();
        let Some(bus_address) = desktop.dbus_session_bus_address.as_ref() else {
            warnings.push("DBUS_SESSION_BUS_ADDRESS is not set; desktop portals disabled".into());
            return Ok(Self {
                proxy: None,
                mode: PortalMode::Disabled,
                warnings,
            });
        };

        let (portal_helper, status_notifier_helper) = ensure_bridge_helpers(paths)?;
        let app_scope = app_scope_name(app_id);
        let shared_dir = shared_portal_dir(paths, app_id);
        let doc_dir = shared_dir.join("doc");
        let sandbox_doc_dir = sandbox_root
            .join("run")
            .join("user")
            .join(uid.to_string())
            .join("doc");

        let bus_dir = shared_dir.join("bus");
        fs::create_dir_all(paths.portal().join("locks")).context("create portal lock directory")?;
        let lock_path = paths
            .portal()
            .join("locks")
            .join(format!("{app_scope}.lock"));
        let lock = lock_portal_scope(&lock_path)?;
        fs::create_dir_all(&doc_dir).with_context(|| format!("create {}", doc_dir.display()))?;
        fs::create_dir_all(&bus_dir).with_context(|| format!("create {}", bus_dir.display()))?;
        fs::write(shared_dir.join("app-id"), app_id)
            .with_context(|| format!("write portal app scope for {app_id}"))?;
        let bus_socket = bus_dir.join("bus");
        let host_private_bus_address = format!("unix:path={}", bus_socket.display());
        let mountpoint = format!("/run/user/{uid}/doc");
        let grant_store = paths.portal_documents().join(format!("{app_scope}.ini"));
        if !shared_portal_ready(&host_private_bus_address, &mountpoint) {
            stop_shared_portal(&shared_dir)?;
            fs::create_dir_all(&doc_dir)
                .with_context(|| format!("create {}", doc_dir.display()))?;
            fs::create_dir_all(&bus_dir)
                .with_context(|| format!("create {}", bus_dir.display()))?;
            fs::write(shared_dir.join("app-id"), app_id)
                .with_context(|| format!("write portal app scope for {app_id}"))?;
            let bus_config = bus_dir.join("session.conf");
            fs::write(&bus_config, private_bus_config(&bus_socket))
                .with_context(|| format!("write {}", bus_config.display()))?;

            let (mut bus_child, address) = start_private_bus(&bus_config)?;
            fs::write(shared_dir.join("bus.pid"), bus_child.id().to_string())
                .context("write private bus pid")?;
            let app_sandbox_root = paths.chroots().join(app_scope_name(app_id));
            let mut bridge_command = Command::new(&portal_helper);
            bridge_command
                .arg("--app-id")
                .arg(app_id)
                .arg("--doc-dir")
                .arg(&doc_dir)
                .arg("--sandbox-root")
                .arg(&app_sandbox_root)
                .arg("--mountpoint")
                .arg(&mountpoint)
                .arg("--grant-store")
                .arg(&grant_store)
                .env("DBUS_SESSION_BUS_ADDRESS", &address)
                .env("HOST_DBUS_SESSION_BUS_ADDRESS", bus_address)
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
            detach_shared_process(&mut bridge_command);
            let mut bridge_child = bridge_command
                .spawn()
                .with_context(|| format!("start {}", portal_helper.display()))?;
            fs::write(
                shared_dir.join("portal-bridge.pid"),
                bridge_child.id().to_string(),
            )
            .context("write portal bridge pid")?;
            let mut status_command = Command::new(&status_notifier_helper);
            status_command
                .arg("--app-id")
                .arg(app_id)
                .arg("--shared-dir")
                .arg(&shared_dir)
                .arg("--app-root")
                .arg(app.app_dir.join("files"))
                .arg("--runtime-root")
                .arg(app.runtime_dir.join("files"))
                .env("DBUS_SESSION_BUS_ADDRESS", &address)
                .env("HOST_DBUS_SESSION_BUS_ADDRESS", bus_address)
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
            detach_shared_process(&mut status_command);
            let mut status_child = match status_command.spawn() {
                Ok(child) => child,
                Err(error) => {
                    terminate_child(&mut bridge_child);
                    terminate_child(&mut bus_child);
                    return Err(error)
                        .with_context(|| format!("start {}", status_notifier_helper.display()));
                }
            };
            fs::write(
                shared_dir.join("status-notifier-bridge.pid"),
                status_child.id().to_string(),
            )
            .context("write status notifier bridge pid")?;
            if let Err(error) = wait_for_portal_proxy(&address, &mountpoint) {
                terminate_child(&mut status_child);
                terminate_child(&mut bridge_child);
                terminate_child(&mut bus_child);
                return Err(error).context("wait for shared compatibility bridges");
            }
        }
        drop(lock);
        let sandbox_bus_address = sandbox_bus_address(&desktop.xdg_runtime_dir, &bus_socket, uid)
            .with_context(|| {
            format!(
                "map private bus {} into chroot /run/user/{uid}",
                bus_socket.display()
            )
        })?;

        Ok(Self {
            proxy: Some(PortalProxy {
                paths: paths.clone(),
                app_id: app_id.to_string(),
                instance_id: instance_id.to_string(),
                shared_dir,
                doc_dir,
                sandbox_doc_dir,
                private_bus_address: host_private_bus_address,
                sandbox_bus_address,
            }),
            mode: PortalMode::PrivateProxy,
            warnings,
        })
    }

    pub fn env(&self) -> Vec<(String, String)> {
        let mut env = vec![("GTK_USE_PORTAL".to_string(), "1".to_string())];
        if let Some(proxy) = &self.proxy {
            env.push((
                "DBUS_SESSION_BUS_ADDRESS".to_string(),
                proxy.sandbox_bus_address.clone(),
            ));
        }
        env
    }

    pub fn doc_dir(&self) -> Option<&Path> {
        self.proxy.as_ref().map(|proxy| proxy.doc_dir.as_path())
    }

    pub fn attach_sandbox(&self) -> Result<()> {
        let Some(proxy) = &self.proxy else {
            return Ok(());
        };
        portal_control(proxy, "AddSandbox")
    }

    pub fn describe(&self) -> Vec<String> {
        match (&self.mode, &self.proxy) {
            (PortalMode::PrivateProxy, Some(proxy)) => vec![
                format!(
                    "shared app bus: {}",
                    proxy
                        .sandbox_bus_address
                        .strip_prefix("unix:path=")
                        .unwrap_or(&proxy.sandbox_bus_address)
                ),
                format!(
                    "document grants: {} -> /run/user/*/doc",
                    proxy.doc_dir.display()
                ),
                format!(
                    "document mount targets: {}",
                    proxy.sandbox_doc_dir.display()
                ),
            ],
            (PortalMode::Disabled, _) => vec!["disabled".to_string()],
            (PortalMode::PrivateProxy, None) => vec!["private portal proxy stopped".to_string()],
        }
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn cleanup(&mut self) -> Result<()> {
        let Some(proxy) = self.proxy.as_mut() else {
            return Ok(());
        };

        portal_control(proxy, "RemoveSandbox")?;
        if !other_active_app_instances(&proxy.paths, &proxy.app_id, &proxy.instance_id)? {
            let app_scope = app_scope_name(&proxy.app_id);
            let lock_path = proxy
                .paths
                .portal()
                .join("locks")
                .join(format!("{app_scope}.lock"));
            let _lock = lock_portal_scope(&lock_path)?;
            if !other_active_app_instances(&proxy.paths, &proxy.app_id, &proxy.instance_id)? {
                stop_shared_portal(&proxy.shared_dir)?;
            }
        }
        self.proxy = None;
        Ok(())
    }
}
