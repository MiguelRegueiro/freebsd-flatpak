use super::file_argument_translation::{
    host_path_from_arg, is_sandbox_internal_path, is_standalone_desktop_field_code,
    local_file_uri_path, looks_like_path_arg, normalize_absolute_path, percent_encode_path,
    warn_unmapped_file_arg,
};
use super::filesystem_permissions::{
    parse_filesystem_permissions, AccessMode, FilesystemPermission,
};
use anyhow::{bail, Context, Result};
use std::env;
#[cfg(target_os = "freebsd")]
use std::ffi::CStr;
use std::fs;
use std::path::{Component, Path, PathBuf};
#[derive(Debug, Clone)]
pub struct HostPathGrant {
    label: String,
    source_permission: String,
    host_path: PathBuf,
    canonical_host_path: PathBuf,
    sandbox_path: PathBuf,
    access: AccessMode,
}

impl HostPathGrant {
    pub(super) fn new(
        label: impl Into<String>,
        source_permission: impl Into<String>,
        host_path: PathBuf,
        sandbox_path: PathBuf,
        access: AccessMode,
    ) -> Result<Self> {
        if !host_path.is_absolute() {
            bail!(
                "host filesystem grant must be absolute: {}",
                host_path.display()
            );
        }
        if !sandbox_path.is_absolute() {
            bail!(
                "sandbox filesystem grant must be absolute: {}",
                sandbox_path.display()
            );
        }

        let canonical_host_path =
            fs::canonicalize(&host_path).unwrap_or_else(|_| host_path.clone());
        Ok(Self {
            label: label.into(),
            source_permission: source_permission.into(),
            host_path,
            canonical_host_path,
            sandbox_path,
            access,
        })
    }

    pub fn source_permission(&self) -> &str {
        &self.source_permission
    }

    pub fn host_path(&self) -> &Path {
        &self.host_path
    }

    pub fn sandbox_path(&self) -> &Path {
        &self.sandbox_path
    }

    pub fn access(&self) -> AccessMode {
        self.access
    }

    pub fn sandbox_target_relative(&self) -> Result<PathBuf> {
        absolute_to_chroot_relative(&self.sandbox_path)
    }

    fn map_host_path(&self, host_path: &Path) -> Option<PathBuf> {
        if let Ok(suffix) = host_path.strip_prefix(&self.host_path) {
            return Some(self.sandbox_path.join(suffix));
        }
        if let Ok(suffix) = host_path.strip_prefix(&self.canonical_host_path) {
            return Some(self.sandbox_path.join(suffix));
        }
        None
    }

    fn same_mount(&self, host_path: &Path, sandbox_path: &Path) -> bool {
        (self.host_path == host_path || self.canonical_host_path == host_path)
            && self.sandbox_path == sandbox_path
    }
}

#[derive(Debug, Clone)]
pub struct HostFilesystem {
    permissions: Vec<FilesystemPermission>,
    grants: Vec<HostPathGrant>,
    warnings: Vec<String>,
    sandbox_home: PathBuf,
}

impl HostFilesystem {
    pub fn from_metadata_file_for_user(
        metadata_path: &Path,
        user: &str,
        project_root: &Path,
        sandbox_root: &Path,
    ) -> Result<Self> {
        let metadata = fs::read_to_string(metadata_path)
            .with_context(|| format!("read Flatpak metadata {}", metadata_path.display()))?;
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME must be set to derive Flatpak filesystem grants")?;
        Self::from_metadata(&metadata, user, &home, project_root, sandbox_root)
    }

    fn from_metadata(
        metadata: &str,
        user: &str,
        home: &Path,
        project_root: &Path,
        sandbox_root: &Path,
    ) -> Result<Self> {
        Self::from_metadata_with_mount_points(
            metadata,
            user,
            home,
            project_root,
            sandbox_root,
            host_mount_points(),
        )
    }

    fn from_metadata_with_mount_points(
        metadata: &str,
        user: &str,
        home: &Path,
        project_root: &Path,
        sandbox_root: &Path,
        mount_points: Vec<PathBuf>,
    ) -> Result<Self> {
        let permissions = parse_filesystem_permissions(metadata)?;
        let xdg_dirs = XdgUserDirs::load(home);
        let sandbox_home = PathBuf::from("/home").join(user);
        let mut builder = GrantBuilder {
            grants: Vec::new(),
            warnings: Vec::new(),
            mount_points,
            home: home.to_path_buf(),
            sandbox_home,
            project_root: fs::canonicalize(project_root)
                .unwrap_or_else(|_| project_root.to_path_buf()),
            sandbox_root: sandbox_root.to_path_buf(),
            xdg_dirs,
        };

        for permission in &permissions {
            builder.apply_permission(permission)?;
        }

        builder.sort_grants();
        Ok(Self {
            permissions,
            grants: builder.grants,
            warnings: builder.warnings,
            sandbox_home: builder.sandbox_home,
        })
    }

    #[cfg(test)]
    pub(super) fn new_for_tests(grants: Vec<HostPathGrant>) -> Self {
        Self {
            permissions: Vec::new(),
            grants,
            warnings: Vec::new(),
            sandbox_home: PathBuf::from("/home/user"),
        }
    }

    pub fn permissions(&self) -> &[FilesystemPermission] {
        &self.permissions
    }

    pub fn grants(&self) -> &[HostPathGrant] {
        &self.grants
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn translate_args(&self, args: &[String]) -> Result<Vec<String>> {
        let mut translated = Vec::new();
        for arg in args {
            if is_standalone_desktop_field_code(arg) {
                continue;
            }

            if arg.starts_with("file://") {
                translated.push(self.translate_file_uri(arg)?);
            } else if looks_like_path_arg(arg) {
                translated.push(self.translate_path_arg(arg)?);
            } else {
                translated.push(arg.clone());
            }
        }
        Ok(translated)
    }

    pub fn write_xdg_user_dirs_config(&self, chroot_root: &Path) -> Result<()> {
        let path = chroot_root
            .join("var")
            .join("config")
            .join("user-dirs.dirs");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }

        let download = self.sandbox_dir_for_label("xdg-download", "/var/data/Downloads");
        let documents = self.sandbox_dir_for_label("xdg-documents", "/var/data/Documents");
        let pictures = self.sandbox_dir_for_label("xdg-pictures", "/var/data/Pictures");
        let desktop = self.sandbox_dir_for_label("xdg-desktop", "/var/data/Desktop");
        let music = self.sandbox_dir_for_label("xdg-music", "/var/data/Music");
        let public_share = self.sandbox_dir_for_label("xdg-public-share", "/var/data/Public");
        let videos = self.sandbox_dir_for_label("xdg-videos", "/var/data/Videos");
        let data = format!(
            "\
# Generated by freebsd-flatpak-poc for this sandbox root.
XDG_DESKTOP_DIR=\"{desktop}\"
XDG_DOWNLOAD_DIR=\"{download}\"
XDG_TEMPLATES_DIR=\"/var/data/Templates\"
XDG_PUBLICSHARE_DIR=\"{public_share}\"
XDG_DOCUMENTS_DIR=\"{documents}\"
XDG_MUSIC_DIR=\"{music}\"
XDG_PICTURES_DIR=\"{pictures}\"
XDG_VIDEOS_DIR=\"{videos}\"
"
        );
        fs::write(&path, data).with_context(|| format!("write {}", path.display()))
    }

    pub fn user_dir_env(&self) -> Vec<(String, String)> {
        let mut env = Vec::new();
        for (label, key) in [
            ("xdg-desktop", "XDG_DESKTOP_DIR"),
            ("xdg-download", "XDG_DOWNLOAD_DIR"),
            ("xdg-documents", "XDG_DOCUMENTS_DIR"),
            ("xdg-music", "XDG_MUSIC_DIR"),
            ("xdg-pictures", "XDG_PICTURES_DIR"),
            ("xdg-public-share", "XDG_PUBLICSHARE_DIR"),
            ("xdg-videos", "XDG_VIDEOS_DIR"),
        ] {
            if let Some(path) = self
                .grants
                .iter()
                .find(|grant| grant.label == label)
                .map(|grant| grant.sandbox_path.display().to_string())
            {
                env.push((key.to_string(), path));
            }
        }
        env
    }

    pub fn sandbox_home_env(&self, fallback: &str) -> String {
        if self.grants.iter().any(|grant| {
            grant.sandbox_path == self.sandbox_home
                || grant.sandbox_path.starts_with(&self.sandbox_home)
                    && (grant.label == "home" || grant.source_permission == "host")
        }) {
            self.sandbox_home.display().to_string()
        } else {
            fallback.to_string()
        }
    }

    fn translate_file_uri(&self, arg: &str) -> Result<String> {
        let host_path = local_file_uri_path(arg)?;
        if is_sandbox_internal_path(&host_path) {
            return Ok(arg.to_string());
        }

        let Some(sandbox_path) = self.map_host_path(&host_path)? else {
            warn_unmapped_file_arg(arg);
            return Ok(arg.to_string());
        };
        Ok(format!("file://{}", percent_encode_path(&sandbox_path)))
    }

    fn translate_path_arg(&self, arg: &str) -> Result<String> {
        let host_path = host_path_from_arg(arg)?;
        if is_sandbox_internal_path(&host_path) {
            return Ok(arg.to_string());
        }

        let Some(sandbox_path) = self.map_host_path(&host_path)? else {
            warn_unmapped_file_arg(arg);
            return Ok(arg.to_string());
        };
        Ok(sandbox_path.display().to_string())
    }

    fn map_host_path(&self, host_path: &Path) -> Result<Option<PathBuf>> {
        let normalized = normalize_absolute_path(host_path)?;
        let mut best_match: Option<PathBuf> = None;

        for grant in &self.grants {
            if let Some(mapped) = grant.map_host_path(&normalized) {
                let replace = best_match
                    .as_ref()
                    .map(|current| mapped.components().count() > current.components().count())
                    .unwrap_or(true);
                if replace {
                    best_match = Some(mapped);
                }
            }
        }

        Ok(best_match)
    }

    fn sandbox_dir_for_label(&self, label: &str, fallback: &str) -> String {
        self.grants
            .iter()
            .find(|grant| grant.label == label)
            .map(|grant| grant.sandbox_path.display().to_string())
            .unwrap_or_else(|| fallback.to_string())
    }
}

#[derive(Debug)]
struct GrantBuilder {
    grants: Vec<HostPathGrant>,
    warnings: Vec<String>,
    mount_points: Vec<PathBuf>,
    home: PathBuf,
    sandbox_home: PathBuf,
    project_root: PathBuf,
    sandbox_root: PathBuf,
    xdg_dirs: XdgUserDirs,
}

impl GrantBuilder {
    fn apply_permission(&mut self, permission: &FilesystemPermission) -> Result<()> {
        let (base, suffix) = permission
            .path
            .split_once('/')
            .map(|(base, suffix)| (base, Some(suffix)))
            .unwrap_or((permission.path.as_str(), None));

        match base {
            "host" if suffix.is_none() => self.add_host_grants(permission),
            "home" | "~" => {
                let host_path = append_permission_suffix(&self.home, suffix)?;
                let sandbox_path = append_permission_suffix(&self.sandbox_home, suffix)?;
                self.add_grant_or_expand(
                    permission,
                    "home",
                    host_path,
                    sandbox_path,
                    permission.create,
                    0,
                )
            }
            "xdg-desktop" | "xdg-documents" | "xdg-download" | "xdg-music" | "xdg-pictures"
            | "xdg-public-share" | "xdg-videos" => {
                let Some(host_base) = self.xdg_dirs.get(base) else {
                    self.warnings
                        .push(format!("unsupported XDG filesystem permission: {base}"));
                    return Ok(());
                };
                let host_path = append_permission_suffix(host_base, suffix)?;
                let sandbox_path = host_path.clone();
                self.add_grant_or_expand(
                    permission,
                    base,
                    host_path,
                    sandbox_path,
                    permission.create,
                    0,
                )
            }
            _ => {
                self.warnings.push(format!(
                    "skipping unsupported filesystem permission {}",
                    permission.original
                ));
                Ok(())
            }
        }
    }

    fn add_host_grants(&mut self, permission: &FilesystemPermission) -> Result<()> {
        let roots = [
            PathBuf::from("/home"),
            PathBuf::from("/media"),
            PathBuf::from("/mnt"),
            PathBuf::from("/opt"),
            PathBuf::from("/run/media"),
            PathBuf::from("/srv"),
        ];
        for path in roots {
            if path.is_dir() {
                self.add_grant_or_expand(
                    permission,
                    "host",
                    path.clone(),
                    path,
                    permission.create,
                    0,
                )?;
            }
        }
        Ok(())
    }

    fn add_grant_or_expand(
        &mut self,
        permission: &FilesystemPermission,
        label: &str,
        host_path: PathBuf,
        sandbox_path: PathBuf,
        create: bool,
        depth: usize,
    ) -> Result<()> {
        if depth == 0 {
            for granted_path in
                authorized_grant_paths(std::slice::from_ref(&host_path), &self.mount_points)
            {
                let suffix = granted_path
                    .strip_prefix(&host_path)
                    .with_context(|| {
                        format!(
                            "map subordinate mount {} below grant {}",
                            granted_path.display(),
                            host_path.display()
                        )
                    })?
                    .to_path_buf();
                self.add_grant_or_expand(
                    permission,
                    label,
                    granted_path,
                    sandbox_path.join(&suffix),
                    create && suffix.as_os_str().is_empty(),
                    1,
                )?;
            }
            return Ok(());
        }

        if create && !host_path.exists() {
            fs::create_dir_all(&host_path)
                .with_context(|| format!("create host directory {}", host_path.display()))?;
        }

        let Ok(host_path) = fs::canonicalize(&host_path) else {
            self.warnings.push(format!(
                "skipping missing host path for {}: {}",
                permission.original,
                host_path.display()
            ));
            return Ok(());
        };

        if !host_path.is_dir() {
            self.warnings.push(format!(
                "skipping non-directory host path for {}: {}",
                permission.original,
                host_path.display()
            ));
            return Ok(());
        }

        if host_path.starts_with(&self.project_root) {
            self.warnings.push(format!(
                "skipping project directory for {} to avoid recursive nullfs mount: {}",
                permission.original,
                host_path.display()
            ));
            return Ok(());
        }

        let chroot_target = self
            .sandbox_root
            .join(absolute_to_chroot_relative(&sandbox_path)?);
        if path_overlaps(&host_path, &chroot_target) {
            if depth >= 8 {
                self.warnings.push(format!(
                    "skipping deeply recursive host path for {}: {}",
                    permission.original,
                    host_path.display()
                ));
                return Ok(());
            }
            self.expand_directory_children(
                permission,
                label,
                &host_path,
                &sandbox_path,
                depth + 1,
            )?;
            return Ok(());
        }

        self.add_grant(
            label.to_string(),
            permission.original.clone(),
            host_path,
            sandbox_path,
            permission.access,
        )
    }

    fn expand_directory_children(
        &mut self,
        permission: &FilesystemPermission,
        label: &str,
        host_path: &Path,
        sandbox_path: &Path,
        depth: usize,
    ) -> Result<()> {
        self.warnings.push(format!(
            "expanding {} into child directory mounts to avoid recursive nullfs mount: {}",
            permission.original,
            host_path.display()
        ));
        let mut entries = fs::read_dir(host_path)
            .with_context(|| format!("read host directory {}", host_path.display()))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());

        for entry in entries {
            let metadata = entry
                .metadata()
                .with_context(|| format!("stat {}", entry.path().display()))?;
            if !metadata.is_dir() {
                continue;
            }
            let name = entry.file_name();
            self.add_grant_or_expand(
                permission,
                label,
                entry.path(),
                sandbox_path.join(name),
                false,
                depth,
            )?;
        }
        Ok(())
    }

    fn add_grant(
        &mut self,
        label: String,
        source_permission: String,
        host_path: PathBuf,
        sandbox_path: PathBuf,
        access: AccessMode,
    ) -> Result<()> {
        let label = match self.xdg_dirs.label_for_host_path(&host_path) {
            Some(xdg_label) => xdg_label.to_string(),
            None => label,
        };

        if let Some(existing) = self
            .grants
            .iter_mut()
            .find(|grant| grant.same_mount(&host_path, &sandbox_path))
        {
            existing.access = existing.access.merge(access);
            return Ok(());
        }

        self.grants.push(HostPathGrant::new(
            label,
            source_permission,
            host_path,
            sandbox_path,
            access,
        )?);
        Ok(())
    }

    fn sort_grants(&mut self) {
        self.grants.sort_by(|left, right| {
            left.sandbox_path
                .components()
                .count()
                .cmp(&right.sandbox_path.components().count())
                .then_with(|| left.sandbox_path.cmp(&right.sandbox_path))
                .then_with(|| left.host_path.cmp(&right.host_path))
        });
    }
}

fn authorized_grant_paths(roots: &[PathBuf], mount_points: &[PathBuf]) -> Vec<PathBuf> {
    // A FreeBSD nullfs mount of a parent does not expose filesystems mounted
    // below it. Mount each nested filesystem at the corresponding sandbox
    // path so every authorized Flatpak grant has the same view as the host
    // process without widening the grant root.
    let mut paths = roots.to_vec();
    paths.extend(
        mount_points
            .iter()
            .filter(|mount| clean_absolute_path(mount))
            .filter(|mount| {
                roots
                    .iter()
                    .any(|root| *mount != root && mount.starts_with(root))
            })
            .cloned(),
    );
    paths.sort_by(|left, right| {
        left.components()
            .count()
            .cmp(&right.components().count())
            .then_with(|| left.cmp(right))
    });
    paths.dedup();
    paths
}

fn clean_absolute_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
}

#[cfg(target_os = "freebsd")]
fn host_mount_points() -> Vec<PathBuf> {
    let mut entries: *mut libc::statfs = std::ptr::null_mut();
    let count = unsafe { libc::getmntinfo(&mut entries, libc::MNT_NOWAIT) };
    if count <= 0 || entries.is_null() {
        return Vec::new();
    }

    let mounts = unsafe { std::slice::from_raw_parts(entries, count as usize) };
    mounts
        .iter()
        .filter_map(|mount| {
            let path = unsafe { CStr::from_ptr(mount.f_mntonname.as_ptr()) };
            path.to_str().ok().map(PathBuf::from)
        })
        .collect()
}

#[cfg(not(target_os = "freebsd"))]
fn host_mount_points() -> Vec<PathBuf> {
    Vec::new()
}

#[derive(Debug, Clone)]
struct XdgUserDirs {
    desktop: PathBuf,
    documents: PathBuf,
    download: PathBuf,
    music: PathBuf,
    pictures: PathBuf,
    public_share: PathBuf,
    videos: PathBuf,
}

impl XdgUserDirs {
    fn load(home: &Path) -> Self {
        let mut dirs = Self {
            desktop: home.join("Desktop"),
            documents: home.join("Documents"),
            download: home.join("Downloads"),
            music: home.join("Music"),
            pictures: home.join("Pictures"),
            public_share: home.join("Public"),
            videos: home.join("Videos"),
        };

        let config_home = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let user_dirs = config_home.join("user-dirs.dirs");
        let Ok(data) = fs::read_to_string(&user_dirs) else {
            return dirs;
        };

        for line in data.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, raw_value)) = line.split_once('=') else {
                continue;
            };
            let Some(path) = parse_xdg_user_dir_value(raw_value.trim(), home) else {
                continue;
            };
            match key.trim() {
                "XDG_DESKTOP_DIR" => dirs.desktop = path,
                "XDG_DOCUMENTS_DIR" => dirs.documents = path,
                "XDG_DOWNLOAD_DIR" => dirs.download = path,
                "XDG_MUSIC_DIR" => dirs.music = path,
                "XDG_PICTURES_DIR" => dirs.pictures = path,
                "XDG_PUBLICSHARE_DIR" => dirs.public_share = path,
                "XDG_VIDEOS_DIR" => dirs.videos = path,
                _ => {}
            }
        }

        dirs
    }

    fn get(&self, name: &str) -> Option<&PathBuf> {
        match name {
            "xdg-desktop" => Some(&self.desktop),
            "xdg-documents" => Some(&self.documents),
            "xdg-download" => Some(&self.download),
            "xdg-music" => Some(&self.music),
            "xdg-pictures" => Some(&self.pictures),
            "xdg-public-share" => Some(&self.public_share),
            "xdg-videos" => Some(&self.videos),
            _ => None,
        }
    }

    fn label_for_host_path(&self, path: &Path) -> Option<&'static str> {
        for (label, candidate) in [
            ("xdg-desktop", &self.desktop),
            ("xdg-documents", &self.documents),
            ("xdg-download", &self.download),
            ("xdg-music", &self.music),
            ("xdg-pictures", &self.pictures),
            ("xdg-public-share", &self.public_share),
            ("xdg-videos", &self.videos),
        ] {
            if fs::canonicalize(candidate).unwrap_or_else(|_| candidate.clone()) == path {
                return Some(label);
            }
        }
        None
    }
}

fn append_permission_suffix(base: &Path, suffix: Option<&str>) -> Result<PathBuf> {
    let mut path = base.to_path_buf();
    let Some(suffix) = suffix else {
        return Ok(path);
    };
    for component in Path::new(suffix).components() {
        match component {
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            _ => bail!("invalid filesystem permission subpath: {suffix}"),
        }
    }
    Ok(path)
}

fn parse_xdg_user_dir_value(value: &str, home: &Path) -> Option<PathBuf> {
    let unquoted = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value);
    let unescaped = unescape_xdg_value(unquoted);
    if let Some(rest) = unescaped.strip_prefix("$HOME/") {
        return Some(home.join(rest));
    }
    if unescaped == "$HOME" {
        return Some(home.to_path_buf());
    }
    if let Some(rest) = unescaped.strip_prefix("${HOME}/") {
        return Some(home.join(rest));
    }
    if unescaped == "${HOME}" {
        return Some(home.to_path_buf());
    }
    let path = PathBuf::from(unescaped);
    if path.is_absolute() {
        Some(path)
    } else {
        None
    }
}

fn unescape_xdg_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut escaped = false;
    for ch in value.chars() {
        if escaped {
            out.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else {
            out.push(ch);
        }
    }
    if escaped {
        out.push('\\');
    }
    out
}

fn absolute_to_chroot_relative(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("sandbox path must be absolute: {}", path.display());
    }

    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            _ => bail!("invalid sandbox path: {}", path.display()),
        }
    }

    if relative.as_os_str().is_empty() {
        bail!("refusing to use sandbox root as a mount target");
    }
    Ok(relative)
}

fn path_overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

#[cfg(test)]
#[path = "tests/filesystem_grants.rs"]
mod tests;
