use crate::installation::installation_paths::Installation;
use crate::ostree::Storage;
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const DEFAULT_REMOTE: &str = "flathub";
const FLATHUB_URL: &str = "https://dl.flathub.org/repo";
const FLATHUB_TITLE: &str = "Flathub";
const FLATHUB_GPG_KEY_BASE64: &str = include_str!("../../vendor/flathub.gpg.base64");
const BOOTSTRAP_MARKER: &str = ".initialized-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub url: String,
    pub title: Option<String>,
    pub enabled: bool,
    pub gpg_verify: bool,
    pub gpg_key: Option<String>,
}

impl Remote {
    fn flathub() -> Self {
        Self {
            name: DEFAULT_REMOTE.to_string(),
            url: FLATHUB_URL.to_string(),
            title: Some(FLATHUB_TITLE.to_string()),
            enabled: true,
            gpg_verify: true,
            gpg_key: Some(FLATHUB_GPG_KEY_BASE64.trim().to_string()),
        }
    }
}

pub fn initialize(paths: &Installation) -> Result<()> {
    fs::create_dir_all(paths.remote_configs()).context("create remote configuration directory")?;
    let marker = paths.remote_configs().join(BOOTSTRAP_MARKER);
    if !marker.exists() {
        if list(paths)?.is_empty() {
            write(paths, &Remote::flathub())?;
        }
        fs::write(&marker, b"1\n").with_context(|| format!("write {}", marker.display()))?;
    }
    let remotes = list(paths)?;
    let storage = Storage::open(paths)?;
    for remote in &remotes {
        storage.configure_remote(remote)?;
    }
    Ok(())
}

pub fn list(paths: &Installation) -> Result<Vec<Remote>> {
    let root = paths.remote_configs();
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut remotes = Vec::new();
    for entry in fs::read_dir(&root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry.path().extension().and_then(|value| value.to_str()) == Some("conf")
        {
            remotes.push(read_path(&entry.path())?);
        }
    }
    remotes.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(remotes)
}

pub fn enabled(paths: &Installation) -> Result<Vec<Remote>> {
    Ok(list(paths)?
        .into_iter()
        .filter(|remote| remote.enabled)
        .collect())
}

pub fn get(paths: &Installation, name: &str) -> Result<Remote> {
    validate_name(name)?;
    let path = config_path(paths, name);
    read_path(&path).with_context(|| format!("remote is not configured: {name}"))
}

pub fn add(paths: &Installation, remote: &Remote, if_not_exists: bool) -> Result<bool> {
    validate_remote(remote)?;
    let path = config_path(paths, &remote.name);
    if path.exists() {
        if if_not_exists {
            return Ok(false);
        }
        bail!("remote already exists: {}", remote.name);
    }
    write(paths, remote)?;
    if let Err(error) = Storage::open(paths)?.configure_remote(remote) {
        let _ = fs::remove_file(&path);
        if let Ok(storage) = Storage::open(paths) {
            let _ = storage.delete_remote(&remote.name);
        }
        return Err(error);
    }
    Ok(true)
}

pub fn modify(paths: &Installation, remote: &Remote) -> Result<()> {
    let previous = get(paths, &remote.name)?;
    validate_remote(remote)?;
    write(paths, remote)?;
    if let Err(error) = Storage::open(paths)?.configure_remote(remote) {
        let _ = write(paths, &previous);
        return Err(error);
    }
    if previous.url != remote.url
        || previous.gpg_verify != remote.gpg_verify
        || previous.gpg_key != remote.gpg_key
    {
        let cache = paths.remote_metadata(&remote.name);
        if cache.exists() {
            fs::remove_dir_all(&cache)
                .with_context(|| format!("invalidate metadata cache {}", cache.display()))?;
        }
    }
    Ok(())
}

pub fn delete(paths: &Installation, name: &str) -> Result<()> {
    get(paths, name)?;
    Storage::open(paths)?.delete_remote(name)?;
    fs::remove_file(config_path(paths, name)).with_context(|| format!("delete remote {name}"))?;
    let cache = paths.remote_metadata(name);
    if cache.exists() {
        fs::remove_dir_all(&cache).with_context(|| format!("remove metadata cache for {name}"))?;
    }
    Ok(())
}

pub fn from_location(name: String, location: &str) -> Result<Remote> {
    validate_name(&name)?;
    if location.ends_with(".flatpakrepo") {
        return from_flatpakrepo(name, location);
    }
    validate_url(location)?;
    Ok(Remote {
        name,
        url: normalize_url(location),
        title: None,
        enabled: true,
        gpg_verify: true,
        gpg_key: None,
    })
}

pub fn read_gpg_key(location: &str) -> Result<String> {
    let data = read_location(location, "GPG key")?;
    Ok(base64::encode(data))
}

fn from_flatpakrepo(name: String, location: &str) -> Result<Remote> {
    let data = read_location(location, "Flatpak repository file")?;
    let text = String::from_utf8(data).context("Flatpak repository file is not UTF-8")?;
    let values = parse_flatpakrepo(&text)?;
    let url = values
        .get("Url")
        .cloned()
        .context("Flatpak repository file has no Url")?;
    validate_url(&url)?;
    let gpg_key = values
        .get("GPGKey")
        .filter(|value| !value.is_empty())
        .cloned();
    Ok(Remote {
        name,
        url: normalize_url(&url),
        title: values
            .get("Title")
            .filter(|value| !value.is_empty())
            .cloned(),
        enabled: true,
        gpg_verify: gpg_key.is_some(),
        gpg_key,
    })
}

fn read_location(location: &str, label: &str) -> Result<Vec<u8>> {
    if let Some(path) = location.strip_prefix("file://") {
        return fs::read(path).with_context(|| format!("read {label} {path}"));
    }
    let path = Path::new(location);
    if path.is_file() {
        return fs::read(path).with_context(|| format!("read {label} {}", path.display()));
    }
    let temp = std::env::temp_dir().join(format!(
        "freebsd-flatpak-remote-{}-{}",
        std::process::id(),
        crate::remotes::unique_sequence()
    ));
    let status = Command::new("fetch")
        .args(["-a", "-F", "-q", "-o"])
        .arg(&temp)
        .arg(location)
        .status()
        .with_context(|| format!("download {label}"))?;
    if !status.success() {
        bail!("download {label} failed with status {status}");
    }
    let result = fs::read(&temp).with_context(|| format!("read downloaded {label}"));
    let _ = fs::remove_file(temp);
    result
}

fn parse_flatpakrepo(text: &str) -> Result<BTreeMap<String, String>> {
    let mut in_group = false;
    let mut values = BTreeMap::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_group = line == "[Flatpak Repo]";
            continue;
        }
        if in_group {
            if let Some((key, value)) = line.split_once('=') {
                values.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
    }
    if values.is_empty() {
        bail!("invalid .flatpakrepo file: missing [Flatpak Repo] group");
    }
    Ok(values)
}

fn write(paths: &Installation, remote: &Remote) -> Result<()> {
    fs::create_dir_all(paths.remote_configs())?;
    let data = format!(
        "name={}\nurl={}\ntitle={}\nenabled={}\ngpg_verify={}\ngpg_key={}\n",
        remote.name,
        remote.url,
        remote.title.as_deref().unwrap_or(""),
        remote.enabled,
        remote.gpg_verify,
        remote.gpg_key.as_deref().unwrap_or("")
    );
    crate::installation::write_state_atomic(&config_path(paths, &remote.name), data.as_bytes())
}

fn read_path(path: &Path) -> Result<Remote> {
    let data = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut values = BTreeMap::new();
    for line in data.lines() {
        if let Some((key, value)) = line.split_once('=') {
            values.insert(key, value);
        }
    }
    let value = |key: &str| {
        values
            .get(key)
            .copied()
            .with_context(|| format!("remote configuration missing {key}"))
    };
    let enabled = value("enabled")?
        .parse()
        .context("parse remote enabled setting")?;
    let gpg_verify = value("gpg_verify")?
        .parse()
        .context("parse remote gpg_verify setting")?;
    let title = value("title")?.to_string();
    let gpg_key = value("gpg_key")?.to_string();
    Ok(Remote {
        name: value("name")?.to_string(),
        url: value("url")?.to_string(),
        title: (!title.is_empty()).then_some(title),
        enabled,
        gpg_verify,
        gpg_key: (!gpg_key.is_empty()).then_some(gpg_key),
    })
}

fn config_path(paths: &Installation, name: &str) -> PathBuf {
    paths.remote_configs().join(format!("{name}.conf"))
}

pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
    {
        bail!("invalid remote name: {name:?}");
    }
    Ok(())
}

fn validate_remote(remote: &Remote) -> Result<()> {
    validate_name(&remote.name)?;
    if remote
        .title
        .as_deref()
        .is_some_and(|title| title.contains(['\n', '\r']))
    {
        bail!("remote title must be a single line");
    }
    validate_url(&remote.url)
}

fn validate_url(url: &str) -> Result<()> {
    if url.chars().any(char::is_whitespace) {
        bail!("remote URL must not contain whitespace: {url:?}");
    }
    if !(url.starts_with("https://") || url.starts_with("http://") || url.starts_with("file://")) {
        bail!("remote location must be an http, https, or file URL: {url}");
    }
    Ok(())
}

fn normalize_url(url: &str) -> String {
    url.trim_end_matches('/').to_string()
}

#[cfg(test)]
#[path = "tests/remote_config.rs"]
mod tests;
