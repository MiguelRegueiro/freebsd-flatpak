use anyhow::{bail, Context, Result};
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    ReadOnly,
    ReadWrite,
}

impl AccessMode {
    pub fn is_read_only(self) -> bool {
        self == Self::ReadOnly
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::ReadWrite => "read-write",
        }
    }

    fn merge(self, other: Self) -> Self {
        if self == Self::ReadWrite || other == Self::ReadWrite {
            Self::ReadWrite
        } else {
            Self::ReadOnly
        }
    }
}

#[derive(Debug, Clone)]
pub struct FilesystemPermission {
    original: String,
    path: String,
    access: AccessMode,
    create: bool,
}

impl FilesystemPermission {
    pub fn original(&self) -> &str {
        &self.original
    }

    pub fn access(&self) -> AccessMode {
        self.access
    }

    pub fn create(&self) -> bool {
        self.create
    }
}

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
    fn new(
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
        let permissions = parse_filesystem_permissions(metadata)?;
        let xdg_dirs = XdgUserDirs::load(home);
        let sandbox_home = PathBuf::from("/home").join(user);
        let mut builder = GrantBuilder {
            grants: Vec::new(),
            warnings: Vec::new(),
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
    fn new_for_tests(grants: Vec<HostPathGrant>) -> Self {
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
        for path in [
            PathBuf::from("/home"),
            PathBuf::from("/media"),
            PathBuf::from("/mnt"),
            PathBuf::from("/opt"),
            PathBuf::from("/run/media"),
            PathBuf::from("/srv"),
        ] {
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

fn parse_filesystem_permissions(metadata: &str) -> Result<Vec<FilesystemPermission>> {
    let Some(value) = metadata_value(metadata, "Context", "filesystems") else {
        return Ok(Vec::new());
    };
    let mut permissions = Vec::new();

    for raw in value
        .split(';')
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
    {
        if raw.starts_with('!') {
            permissions.push(FilesystemPermission {
                original: raw.to_string(),
                path: raw.to_string(),
                access: AccessMode::ReadOnly,
                create: false,
            });
            continue;
        }

        let (path, access, create) = parse_access_suffix(raw)?;
        permissions.push(FilesystemPermission {
            original: raw.to_string(),
            path: path.to_string(),
            access,
            create,
        });
    }

    Ok(permissions)
}

fn parse_access_suffix(raw: &str) -> Result<(&str, AccessMode, bool)> {
    let Some((path, suffix)) = raw.rsplit_once(':') else {
        return Ok((raw, AccessMode::ReadWrite, false));
    };

    match suffix {
        "ro" => Ok((path, AccessMode::ReadOnly, false)),
        "rw" => Ok((path, AccessMode::ReadWrite, false)),
        "create" => Ok((path, AccessMode::ReadWrite, true)),
        _ if path.contains('/') => Ok((raw, AccessMode::ReadWrite, false)),
        _ => bail!("unsupported filesystem permission suffix in {raw}"),
    }
}

fn metadata_value(metadata: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in metadata.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_section = &line[1..line.len() - 1] == section;
            continue;
        }
        if !in_section || line.starts_with('#') || line.is_empty() {
            continue;
        }
        let Some((candidate, value)) = line.split_once('=') else {
            continue;
        };
        if candidate.trim() == key {
            return Some(value.trim().to_string());
        }
    }
    None
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

fn local_file_uri_path(uri: &str) -> Result<PathBuf> {
    let rest = uri
        .strip_prefix("file://")
        .context("file URI must start with file://")?;
    let path_part = if let Some(path) = rest.strip_prefix("localhost/") {
        format!("/{path}")
    } else if rest.starts_with('/') {
        rest.to_string()
    } else {
        bail!("unsupported non-local file URI: {uri}");
    };
    let path_part = path_part
        .split_once('#')
        .map(|(path, _)| path)
        .unwrap_or(&path_part)
        .split_once('?')
        .map(|(path, _)| path)
        .unwrap_or(path_part.as_str());
    Ok(PathBuf::from(percent_decode(path_part)?))
}

fn host_path_from_arg(arg: &str) -> Result<PathBuf> {
    if let Some(rest) = arg.strip_prefix("~/") {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME must be set to expand ~/ file argument")?;
        return Ok(home.join(rest));
    }

    let path = PathBuf::from(arg);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(env::current_dir()
            .context("determine current directory for relative file argument")?
            .join(path))
    }
}

fn normalize_absolute_path(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .context("determine current directory for path normalization")?
            .join(path)
    };

    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::RootDir => normalized.push(Path::new("/")),
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized != Path::new("/") {
                    normalized.pop();
                }
            }
            _ => bail!("unsupported path component in {}", path.display()),
        }
    }

    if normalized.as_os_str().is_empty() {
        normalized.push(Path::new("/"));
    }
    Ok(normalized)
}

fn looks_like_path_arg(arg: &str) -> bool {
    arg.starts_with('/') || arg.starts_with("./") || arg.starts_with("../") || arg.starts_with("~/")
}

fn is_sandbox_internal_path(path: &Path) -> bool {
    [
        "/app", "/bin", "/dev", "/etc", "/lib", "/lib64", "/proc", "/run", "/sys", "/tmp", "/usr",
        "/var",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
}

fn is_standalone_desktop_field_code(arg: &str) -> bool {
    matches!(arg, "%f" | "%F" | "%u" | "%U" | "%i" | "%c" | "%k" | "%%")
}

fn warn_unmapped_file_arg(arg: &str) {
    eprintln!(
        "warning: file argument is outside configured Flatpak filesystem permissions and may not be visible inside sandbox: {arg}"
    );
}

fn path_overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn percent_decode(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                bail!("invalid percent escape in file URI: {value}");
            }
            let hi = hex_value(bytes[index + 1])
                .with_context(|| format!("invalid percent escape in file URI: {value}"))?;
            let lo = hex_value(bytes[index + 2])
                .with_context(|| format!("invalid percent escape in file URI: {value}"))?;
            decoded.push((hi << 4) | lo);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).context("file URI path is not valid UTF-8")
}

fn percent_encode_path(path: &Path) -> String {
    let text = path.display().to_string();
    let mut encoded = String::new();
    for byte in text.bytes() {
        let keep = byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~');
        if keep {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{AccessMode, HostFilesystem, HostPathGrant};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_ID: AtomicUsize = AtomicUsize::new(0);

    struct TestTree {
        root: PathBuf,
    }

    impl TestTree {
        fn new(name: &str) -> Self {
            let id = TEST_ID.fetch_add(1, Ordering::SeqCst);
            let root = std::env::temp_dir().join(format!(
                "freebsd-flatpak-poc-{name}-{}-{id}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.root.join(rel)
        }
    }

    impl Drop for TestTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn test_filesystem() -> HostFilesystem {
        HostFilesystem::new_for_tests(vec![HostPathGrant::new(
            "xdg-documents",
            "xdg-documents",
            PathBuf::from("/host/home/user/Documents"),
            PathBuf::from("/home/user/Documents"),
            AccessMode::ReadWrite,
        )
        .unwrap()])
    }

    fn metadata(filesystems: &str) -> String {
        format!(
            "\
[Application]
name=org.example.App

[Context]
filesystems={filesystems}
"
        )
    }

    #[test]
    fn translates_allowed_absolute_path() {
        let fs = test_filesystem();
        let args = fs
            .translate_args(&["/host/home/user/Documents/test.txt".to_string()])
            .unwrap();
        assert_eq!(args, ["/home/user/Documents/test.txt"]);
    }

    #[test]
    fn translates_allowed_file_uri() {
        let fs = test_filesystem();
        let args = fs
            .translate_args(&["file:///host/home/user/Documents/a%20b.txt".to_string()])
            .unwrap();
        assert_eq!(args, ["file:///home/user/Documents/a%20b.txt"]);
    }

    #[test]
    fn preserves_sandbox_internal_absolute_path() {
        let fs = test_filesystem();
        let args = fs
            .translate_args(&["/var/data/audio-test-tone.wav".to_string()])
            .unwrap();
        assert_eq!(args, ["/var/data/audio-test-tone.wav"]);
    }

    #[test]
    fn preserves_sandbox_internal_file_uri() {
        let fs = test_filesystem();
        let args = fs
            .translate_args(&["file:///var/data/audio-test-tone.wav".to_string()])
            .unwrap();
        assert_eq!(args, ["file:///var/data/audio-test-tone.wav"]);
    }

    #[test]
    fn drops_literal_desktop_field_codes() {
        let fs = test_filesystem();
        let args = fs
            .translate_args(&["--new-window".to_string(), "%U".to_string()])
            .unwrap();
        assert_eq!(args, ["--new-window"]);
    }

    #[test]
    fn metadata_without_filesystems_has_no_grants() {
        let tree = TestTree::new("no-filesystems");
        let home = tree.path("home/user");
        fs::create_dir_all(&home).unwrap();
        let fs = HostFilesystem::from_metadata(
            "[Application]\nname=org.example.App\n",
            "user",
            &home,
            &tree.path("project"),
            &tree.path("project/runtime/chroots/org.example.App"),
        )
        .unwrap();
        assert!(fs.permissions().is_empty());
        assert!(fs.grants().is_empty());
    }

    #[test]
    fn xdg_documents_ro_resolves_read_only() {
        let tree = TestTree::new("xdg-documents-ro");
        let home = tree.path("home/user");
        let documents = home.join("Documents");
        fs::create_dir_all(&documents).unwrap();
        let fs = HostFilesystem::from_metadata(
            &metadata("xdg-documents:ro;"),
            "user",
            &home,
            &tree.path("project"),
            &tree.path("project/runtime/chroots/org.example.App"),
        )
        .unwrap();
        assert_eq!(fs.permissions().len(), 1);
        assert_eq!(fs.grants().len(), 1);
        assert_eq!(fs.grants()[0].host_path(), documents.as_path());
        assert_eq!(fs.grants()[0].sandbox_path(), documents.as_path());
        assert_eq!(fs.grants()[0].access(), AccessMode::ReadOnly);
    }

    #[test]
    fn home_expands_to_children_when_project_lives_under_home() {
        let tree = TestTree::new("home-expands");
        let home = tree.path("home/user");
        let docs = home.join("Documents");
        let project = home.join("freebsd-flatpak-poc");
        fs::create_dir_all(&docs).unwrap();
        fs::create_dir_all(project.join("runtime/chroots/org.example.App")).unwrap();
        let fs = HostFilesystem::from_metadata(
            &metadata("home;"),
            "user",
            &home,
            &project,
            &project.join("runtime/chroots/org.example.App"),
        )
        .unwrap();

        assert!(fs.grants().iter().any(|grant| {
            grant.host_path() == docs.as_path()
                && grant.sandbox_path() == Path::new("/home/user/Documents")
        }));
        assert!(!fs
            .grants()
            .iter()
            .any(|grant| grant.host_path() == project.as_path()));
    }

    #[test]
    fn overlapping_permissions_keep_more_permissive_access() {
        let tree = TestTree::new("overlap");
        let home = tree.path("home/user");
        let documents = home.join("Documents");
        fs::create_dir_all(&documents).unwrap();
        let fs = HostFilesystem::from_metadata(
            &metadata("xdg-documents:ro;xdg-documents;"),
            "user",
            &home,
            &tree.path("project"),
            &tree.path("project/runtime/chroots/org.example.App"),
        )
        .unwrap();
        assert_eq!(fs.grants().len(), 1);
        assert_eq!(fs.grants()[0].access(), AccessMode::ReadWrite);
    }
}
