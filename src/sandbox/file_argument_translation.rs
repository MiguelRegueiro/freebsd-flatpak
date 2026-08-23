use anyhow::{bail, Context, Result};
use std::env;
use std::path::{Component, Path, PathBuf};

pub(super) fn local_file_uri_path(uri: &str) -> Result<PathBuf> {
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

pub(super) fn host_path_from_arg(arg: &str) -> Result<PathBuf> {
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

pub(super) fn normalize_absolute_path(path: &Path) -> Result<PathBuf> {
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

pub(super) fn looks_like_path_arg(arg: &str) -> bool {
    arg.starts_with('/') || arg.starts_with("./") || arg.starts_with("../") || arg.starts_with("~/")
}

pub(super) fn is_sandbox_internal_path(path: &Path) -> bool {
    [
        "/app", "/bin", "/dev", "/etc", "/lib", "/lib64", "/proc", "/run", "/sys", "/tmp", "/usr",
        "/var",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
}

pub(super) fn is_standalone_desktop_field_code(arg: &str) -> bool {
    matches!(arg, "%f" | "%F" | "%u" | "%U" | "%i" | "%c" | "%k" | "%%")
}

pub(super) fn warn_unmapped_file_arg(arg: &str) {
    eprintln!(
        "warning: file argument is outside configured Flatpak filesystem permissions and may not be visible inside sandbox: {arg}"
    );
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

pub(super) fn percent_encode_path(path: &Path) -> String {
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
#[path = "tests/file_argument_translation.rs"]
mod tests;
