use anyhow::{bail, Context, Result};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const APP_DIR_NAME: &str = "freebsd-flatpak";

#[derive(Debug, Clone)]
pub struct Installation {
    data_home: PathBuf,
    data_root: PathBuf,
    cache_root: PathBuf,
    runtime_root: PathBuf,
    app_data_root: PathBuf,
    launcher: PathBuf,
    libexec_root: PathBuf,
}

impl Installation {
    pub fn from_env() -> Result<Self> {
        let home = env_path("HOME").context("HOME must be set")?;
        let data_home = env_path("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share"));
        let cache_home = env_path("XDG_CACHE_HOME").unwrap_or_else(|| home.join(".cache"));
        let runtime_home = match env_path("XDG_RUNTIME_DIR") {
            Some(path) => path,
            None => PathBuf::from(format!("/tmp/freebsd-flatpak-{}", unsafe {
                libc::geteuid()
            })),
        };

        Ok(Self {
            data_root: env_path("FREEBSD_FLATPAK_DATA_DIR")
                .unwrap_or_else(|| data_home.join(APP_DIR_NAME)),
            cache_root: env_path("FREEBSD_FLATPAK_CACHE_DIR")
                .unwrap_or_else(|| cache_home.join(APP_DIR_NAME)),
            runtime_root: env_path("FREEBSD_FLATPAK_RUNTIME_DIR")
                .unwrap_or_else(|| runtime_home.join(APP_DIR_NAME)),
            app_data_root: env_path("FREEBSD_FLATPAK_APP_DATA_DIR")
                .unwrap_or_else(|| home.join(".var/app")),
            launcher: env_path("FREEBSD_FLATPAK_BIN")
                .unwrap_or_else(|| PathBuf::from("/usr/local/bin/flatpak")),
            libexec_root: env_path("FREEBSD_FLATPAK_LIBEXEC_DIR")
                .unwrap_or_else(|| PathBuf::from("/usr/local/libexec/freebsd-flatpak")),
            data_home,
        })
    }

    #[cfg(test)]
    pub fn for_test(root: &Path) -> Self {
        Self {
            data_home: root.join("xdg-data"),
            data_root: root.join("xdg-data/freebsd-flatpak"),
            cache_root: root.join("xdg-cache/freebsd-flatpak"),
            runtime_root: root.join("xdg-runtime/freebsd-flatpak"),
            app_data_root: root.join("home/.var/app"),
            launcher: PathBuf::from("/usr/local/bin/flatpak"),
            libexec_root: root.join("libexec"),
        }
    }

    pub fn ensure(&self) -> Result<()> {
        for path in [
            self.apps(),
            self.runtimes(),
            self.repo(),
            self.refs(),
            self.extensions(),
            self.exports(),
            self.remote_configs(),
            self.remote_metadata_root(),
            self.portal_documents(),
            self.chroots(),
            self.runs(),
            self.portal(),
            self.system_bus(),
            self.gpu(),
            self.app_data_root.clone(),
        ] {
            fs::create_dir_all(&path).with_context(|| format!("create {}", path.display()))?;
        }
        fs::set_permissions(&self.runtime_root, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("secure {}", self.runtime_root.display()))?;
        Ok(())
    }

    pub fn data_home(&self) -> &Path {
        &self.data_home
    }
    pub fn flatpak_overrides(&self) -> PathBuf {
        self.data_home.join("flatpak/overrides")
    }
    pub fn data_root(&self) -> &Path {
        &self.data_root
    }
    pub fn runtime_root(&self) -> &Path {
        &self.runtime_root
    }
    pub fn launcher(&self) -> &Path {
        &self.launcher
    }
    pub fn libexec_root(&self) -> &Path {
        &self.libexec_root
    }

    pub fn apps(&self) -> PathBuf {
        self.data_root.join("apps")
    }
    pub fn app(&self, app_id: &str) -> PathBuf {
        self.apps().join(app_id)
    }
    pub fn runtimes(&self) -> PathBuf {
        self.data_root.join("runtimes")
    }
    pub fn repo(&self) -> PathBuf {
        self.data_root.join("repo")
    }
    pub fn refs(&self) -> PathBuf {
        self.data_root.join("refs")
    }
    pub fn extensions(&self) -> PathBuf {
        self.data_root.join("extensions")
    }
    pub fn exports(&self) -> PathBuf {
        self.data_root.join("exports")
    }
    pub fn export_share(&self) -> PathBuf {
        self.exports().join("share")
    }
    pub fn remote_configs(&self) -> PathBuf {
        self.data_root.join("remotes")
    }
    pub fn remote_metadata_root(&self) -> PathBuf {
        self.cache_root.join("remotes")
    }
    pub fn remote_metadata(&self, remote: &str) -> PathBuf {
        self.remote_metadata_root().join(remote)
    }
    pub fn portal_documents(&self) -> PathBuf {
        self.data_root.join("portal-documents")
    }
    pub fn chroots(&self) -> PathBuf {
        self.runtime_root.join("chroots")
    }
    pub fn runs(&self) -> PathBuf {
        self.runtime_root.join("runs")
    }
    pub fn spawn_brokers(&self) -> PathBuf {
        self.runtime_root.join("spawn-brokers")
    }
    pub fn portal(&self) -> PathBuf {
        self.runtime_root.join("portal")
    }
    pub fn system_bus(&self) -> PathBuf {
        self.runtime_root.join("system-bus")
    }
    pub fn gpu(&self) -> PathBuf {
        self.runtime_root.join("gpu")
    }
    pub fn app_data(&self, app_id: &str) -> Result<PathBuf> {
        validate_app_id(app_id)?;
        Ok(self.app_data_root.join(app_id))
    }

    pub fn relative_data_path(&self, path: &Path) -> Result<PathBuf> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.data_root.join(path)
        };
        absolute
            .strip_prefix(&self.data_root)
            .map(Path::to_path_buf)
            .with_context(|| {
                format!(
                    "{} is outside managed data {}",
                    absolute.display(),
                    self.data_root.display()
                )
            })
    }

    pub fn absolute_data_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.data_root.join(path)
        }
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn validate_app_id(app_id: &str) -> Result<()> {
    if app_id.is_empty() || app_id.contains('/') || app_id == "." || app_id == ".." {
        bail!("invalid app id: {app_id:?}");
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/installation_paths.rs"]
mod tests;
