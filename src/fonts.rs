use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct HostFonts {
    mounts: Vec<FontMount>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FontMount {
    host_path: PathBuf,
    sandbox_path: PathBuf,
}

impl HostFonts {
    pub fn from_host() -> Self {
        let mut warnings = Vec::new();
        let mounts = font_mounts(&mut warnings);
        Self { mounts, warnings }
    }

    pub fn prepare(&self, chroot_root: &Path) -> Result<()> {
        let run_host = chroot_root.join("run/host");
        fs::create_dir_all(&run_host).with_context(|| format!("create {}", run_host.display()))?;
        for dir in [
            "fonts",
            "local-fonts",
            "user-fonts",
            "fonts-cache",
            "user-fonts-cache",
        ] {
            fs::create_dir_all(run_host.join(dir))
                .with_context(|| format!("create {}", run_host.join(dir).display()))?;
        }
        fs::write(run_host.join("font-dirs.xml"), font_dirs_xml())
            .with_context(|| format!("write {}", run_host.join("font-dirs.xml").display()))?;
        Ok(())
    }

    pub fn mounts(&self) -> &[FontMount] {
        &self.mounts
    }

    pub fn describe(&self) -> Vec<String> {
        self.mounts
            .iter()
            .map(|mount| {
                format!(
                    "{} -> {}",
                    mount.host_path.display(),
                    mount.sandbox_path.display()
                )
            })
            .collect()
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

impl FontMount {
    pub fn host_path(&self) -> &Path {
        &self.host_path
    }

    pub fn sandbox_target_relative(&self) -> Result<PathBuf> {
        self.sandbox_path
            .strip_prefix("/")
            .map(Path::to_path_buf)
            .with_context(|| {
                format!(
                    "font sandbox path is not absolute: {}",
                    self.sandbox_path.display()
                )
            })
    }
}

fn font_mounts(warnings: &mut Vec<String>) -> Vec<FontMount> {
    let mut mounts = Vec::new();
    push_mount_if_dir(
        &mut mounts,
        PathBuf::from("/usr/share/fonts"),
        "/run/host/fonts",
    );
    push_mount_if_dir(
        &mut mounts,
        PathBuf::from("/usr/local/share/fonts"),
        "/run/host/local-fonts",
    );

    let user_dirs = user_font_dirs();
    if let Some(path) = user_dirs.iter().find(|path| path.is_dir()) {
        push_mount_if_dir(&mut mounts, path.clone(), "/run/host/user-fonts");
    } else if !user_dirs.is_empty() {
        warnings.push(format!(
            "no user font directory found from: {}",
            user_dirs
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    mounts
}

fn push_mount_if_dir(mounts: &mut Vec<FontMount>, host_path: PathBuf, sandbox_path: &str) {
    if host_path.is_dir() {
        mounts.push(FontMount {
            host_path,
            sandbox_path: PathBuf::from(sandbox_path),
        });
    }
}

fn user_font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(path) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
        push_unique(&mut dirs, &mut seen, path.join("fonts"));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        push_unique(&mut dirs, &mut seen, home.join(".local/share/fonts"));
        push_unique(&mut dirs, &mut seen, home.join(".fonts"));
    }
    dirs
}

fn push_unique(dirs: &mut Vec<PathBuf>, seen: &mut BTreeSet<PathBuf>, path: PathBuf) {
    if seen.insert(path.clone()) {
        dirs.push(path);
    }
}

fn font_dirs_xml() -> &'static str {
    r#"<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
<fontconfig>
  <dir>/run/host/fonts</dir>
  <dir>/run/host/local-fonts</dir>
  <dir>/run/host/user-fonts</dir>
</fontconfig>
"#
}

#[cfg(test)]
mod tests {
    use super::font_dirs_xml;

    #[test]
    fn generated_font_dirs_xml_points_at_flatpak_host_paths() {
        let xml = font_dirs_xml();
        assert!(xml.contains("/run/host/fonts"));
        assert!(xml.contains("/run/host/local-fonts"));
        assert!(xml.contains("/run/host/user-fonts"));
    }
}
