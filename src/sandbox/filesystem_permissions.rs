use crate::flatpak_metadata::value;
use anyhow::{bail, Result};

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

    pub(super) fn merge(self, other: Self) -> Self {
        if self == Self::ReadWrite || other == Self::ReadWrite {
            Self::ReadWrite
        } else {
            Self::ReadOnly
        }
    }
}

#[derive(Debug, Clone)]
pub struct FilesystemPermission {
    pub(super) original: String,
    pub(super) path: String,
    pub(super) access: AccessMode,
    pub(super) create: bool,
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

pub(super) fn parse_filesystem_permissions(metadata: &str) -> Result<Vec<FilesystemPermission>> {
    let Some(value) = value(metadata, "Context", "filesystems") else {
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
