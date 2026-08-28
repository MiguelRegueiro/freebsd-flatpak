use crate::installation as runtime;
use anyhow::{Context, Result};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const UNAVAILABLE_PIPEWIRE_REMOTE: &str = "freebsd-flatpak-no-audio";

#[derive(Debug, Clone)]
pub struct HostAudio {
    sockets: Vec<String>,
    pulse: Option<PulseAudio>,
    pipewire: Option<PipeWireAudio>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct PulseAudio {
    host_socket: PathBuf,
    sandbox_server: String,
    host_cookie: Option<PathBuf>,
    sandbox_cookie: PathBuf,
    sandbox_config: PathBuf,
}

#[derive(Debug, Clone)]
struct PipeWireAudio {
    host_socket: PathBuf,
    remote: String,
}

impl HostAudio {
    pub(crate) fn from_metadata(metadata: &str, xdg_runtime_dir: &Path, uid: u32) -> Self {
        let sockets = parse_socket_permissions(metadata);
        let mut warnings = Vec::new();

        let pulse = if sockets.iter().any(|socket| socket == "pulseaudio") {
            let host_socket = xdg_runtime_dir.join("pulse").join("native");
            if host_socket.exists() {
                let host_cookie = pulse_cookie_path();
                if host_cookie.as_ref().is_some_and(|path| !path.is_file()) {
                    warnings.push(format!(
                        "PulseAudio cookie not found at {}; relying on same-UID socket auth",
                        host_cookie.as_ref().unwrap().display()
                    ));
                }
                Some(PulseAudio {
                    host_socket,
                    sandbox_server: format!("unix:/run/user/{uid}/pulse/native"),
                    host_cookie: host_cookie.filter(|path| path.is_file()),
                    sandbox_cookie: PathBuf::from("/var/config/pulse/cookie"),
                    sandbox_config: PathBuf::from("/var/config/pulse/client.conf"),
                })
            } else {
                warnings.push(format!(
                    "metadata requests pulseaudio but host socket is missing: {}",
                    host_socket.display()
                ));
                None
            }
        } else {
            None
        };

        let pipewire = if sockets.iter().any(|socket| socket == "pipewire") {
            let host_socket = xdg_runtime_dir.join("pipewire-0");
            if host_socket.exists() {
                Some(PipeWireAudio {
                    host_socket,
                    remote: "pipewire-0".to_string(),
                })
            } else {
                warnings.push(format!(
                    "metadata requests pipewire but host socket is missing: {}",
                    host_socket.display()
                ));
                None
            }
        } else {
            None
        };

        Self {
            sockets,
            pulse,
            pipewire,
            warnings,
        }
    }

    pub fn sockets(&self) -> &[String] {
        &self.sockets
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    #[cfg(test)]
    fn has_audio_bridge(&self) -> bool {
        self.pulse.is_some() || self.pipewire.is_some()
    }

    pub fn describe(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(pulse) = &self.pulse {
            lines.push(format!(
                "pulseaudio: {} -> {}",
                pulse.host_socket.display(),
                pulse.sandbox_server
            ));
            if let Some(cookie) = &pulse.host_cookie {
                lines.push(format!(
                    "pulseaudio cookie: {} -> {}",
                    cookie.display(),
                    pulse.sandbox_cookie.display()
                ));
            }
        }
        if let Some(pipewire) = &self.pipewire {
            lines.push(format!(
                "pipewire: {} -> /run/user/*/{}",
                pipewire.host_socket.display(),
                pipewire.remote
            ));
        }
        lines
    }

    pub fn prepare(&self, chroot_root: &Path) -> Result<()> {
        if let Some(pulse) = &self.pulse {
            let config = chroot_root.join(relative_chroot_path(&pulse.sandbox_config));
            let cookie = chroot_root.join(relative_chroot_path(&pulse.sandbox_cookie));
            fs::create_dir_all(
                config
                    .parent()
                    .context("PulseAudio config path has no parent")?,
            )
            .with_context(|| format!("create {}", config.parent().unwrap().display()))?;
            fs::write(
                &config,
                format!(
                    "default-server = {}\nautospawn = no\n",
                    pulse.sandbox_server
                ),
            )
            .with_context(|| format!("write {}", config.display()))?;

            if let Some(host_cookie) = &pulse.host_cookie {
                fs::copy(host_cookie, &cookie)
                    .with_context(|| format!("copy PulseAudio cookie to {}", cookie.display()))?;
                fs::set_permissions(&cookie, fs::Permissions::from_mode(0o600))
                    .with_context(|| format!("set permissions on {}", cookie.display()))?;
            }
        }

        Ok(())
    }

    pub fn cleanup(&self, chroot_root: &Path) -> Result<()> {
        if let Some(pulse) = &self.pulse {
            for path in [&pulse.sandbox_config, &pulse.sandbox_cookie] {
                let host_path = chroot_root.join(relative_chroot_path(path));
                if host_path.exists() {
                    fs::remove_file(&host_path)
                        .with_context(|| format!("remove {}", host_path.display()))?;
                }
            }
        }
        Ok(())
    }

    pub fn env(&self) -> Vec<(String, String)> {
        let mut env = Vec::new();
        if let Some(pulse) = &self.pulse {
            env.push(("PULSE_SERVER".to_string(), pulse.sandbox_server.clone()));
            if pulse.host_cookie.is_some() {
                env.push((
                    "PULSE_COOKIE".to_string(),
                    pulse.sandbox_cookie.display().to_string(),
                ));
            }
        }
        if self.pulse.is_some() {
            // The native FreeBSD PipeWire daemon is used by desktop portals, but without an
            // audio device it must not win backend auto-detection over the working Pulse bridge.
            env.push((
                "PIPEWIRE_REMOTE".to_string(),
                UNAVAILABLE_PIPEWIRE_REMOTE.to_string(),
            ));
        } else if let Some(pipewire) = &self.pipewire {
            env.push(("PIPEWIRE_REMOTE".to_string(), pipewire.remote.clone()));
        }
        env
    }
}

fn parse_socket_permissions(metadata: &str) -> Vec<String> {
    runtime::metadata_value(metadata, "Context", "sockets")
        .map(|value| {
            value
                .split(';')
                .map(str::trim)
                .filter(|socket| !socket.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn pulse_cookie_path() -> Option<PathBuf> {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config_home.join("pulse").join("cookie"))
}

fn relative_chroot_path(path: &Path) -> PathBuf {
    path.strip_prefix("/").unwrap_or(path).to_path_buf()
}

#[cfg(test)]
#[path = "tests/audio.rs"]
mod tests;
