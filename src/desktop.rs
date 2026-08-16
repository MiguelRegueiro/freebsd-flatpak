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
}
