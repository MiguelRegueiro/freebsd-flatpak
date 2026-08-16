use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct DesktopSession {
    pub xdg_runtime_dir: PathBuf,
    pub wayland_display: String,
    pub display: Option<String>,
    pub dbus_session_bus_address: Option<String>,
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
