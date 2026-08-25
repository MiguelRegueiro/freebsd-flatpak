use super::installation_paths::Installation;
use crate::flatpak_metadata::{has_section, section_entries, value};
use anyhow::{bail, Context, Result};
use glib::{Checksum, ChecksumType};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const EXTRA_DATA_SECTION: &str = "Extra Data";
const APPLIED_MARKER: &str = ".extra-data-applied";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExtraDataSource {
    name: String,
    uri: String,
    size: u64,
    checksum: String,
}

pub(crate) fn apply_extra_data(
    paths: &Installation,
    checkout: &Path,
    runtime_dir: &Path,
) -> Result<()> {
    let metadata_path = checkout.join("metadata");
    let metadata = fs::read_to_string(&metadata_path)
        .with_context(|| format!("read {}", metadata_path.display()))?;
    let sources = parse_sources(&metadata)?;
    if sources.is_empty() {
        return Ok(());
    }
    let applied_marker = checkout.join(APPLIED_MARKER);
    if applied_marker.is_file() {
        return Ok(());
    }

    let files = checked_directory(&checkout.join("files"))?;
    let extra = files.join("extra");
    remove_path_if_exists(&extra)?;
    fs::create_dir(&extra).with_context(|| format!("create {}", extra.display()))?;
    let checkout_name = checkout
        .file_name()
        .and_then(|name| name.to_str())
        .context("extra-data checkout has no UTF-8 file name")?;
    let work = checkout
        .parent()
        .context("extra-data checkout has no parent")?
        .join(format!(
            ".{checkout_name}.extra-work-{}",
            std::process::id()
        ));
    remove_path_if_exists(&work)?;
    fs::create_dir(&work).with_context(|| format!("create {}", work.display()))?;

    for source in &sources {
        if let Err(error) = download_source(source, &work) {
            let _ = remove_path_if_exists(&work);
            return Err(error);
        }
    }

    let no_runtime = value(&metadata, EXTRA_DATA_SECTION, "NoRuntime")
        .is_some_and(|setting| setting.eq_ignore_ascii_case("true"));
    if checkout.join("files/bin/apply_extra").is_file() {
        if let Err(error) =
            crate::sandbox::apply_extra::run(paths, checkout, runtime_dir, &work, !no_runtime)
                .context("run Flatpak apply_extra")
        {
            let _ = remove_path_if_exists(&work);
            return Err(error);
        }
    }
    remove_path_if_exists(&extra)?;
    fs::rename(&work, &extra).with_context(|| {
        format!(
            "publish applied extra data {} to {}",
            work.display(),
            extra.display()
        )
    })?;
    fs::write(&applied_marker, b"1\n")
        .with_context(|| format!("mark extra data applied at {}", applied_marker.display()))
}

fn parse_sources(metadata: &str) -> Result<Vec<ExtraDataSource>> {
    if !has_section(metadata, EXTRA_DATA_SECTION) {
        return Ok(Vec::new());
    }
    let entries = section_entries(metadata, EXTRA_DATA_SECTION)
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let mut suffixes = entries
        .keys()
        .filter_map(|key| {
            key.strip_prefix("name")
                .filter(|suffix| suffix.chars().all(|ch| ch.is_ascii_digit()))
                .map(ToOwned::to_owned)
        })
        .collect::<Vec<_>>();
    suffixes.sort_by_key(|suffix| {
        if suffix.is_empty() {
            0
        } else {
            suffix.parse::<u64>().unwrap_or(u64::MAX)
        }
    });
    suffixes.dedup();
    if suffixes.is_empty() {
        bail!("[Extra Data] has no data sources");
    }

    suffixes
        .into_iter()
        .map(|suffix| {
            let field = |name: &str| -> Result<String> {
                entries
                    .get(&format!("{name}{suffix}"))
                    .cloned()
                    .with_context(|| format!("[Extra Data] is missing {name}{suffix}"))
            };
            let name = field("name")?;
            validate_name(&name)?;
            let uri = field("uri")?;
            if !uri
                .strip_prefix("http://")
                .or_else(|| uri.strip_prefix("https://"))
                .is_some_and(|rest| !rest.is_empty())
            {
                bail!("unsupported extra-data URI: {uri}");
            }
            let size_text = field("size")?;
            let size = size_text
                .parse::<u64>()
                .with_context(|| format!("invalid extra-data size: {size_text}"))?;
            let checksum = field("checksum")?.to_ascii_lowercase();
            if checksum.len() != 64 || !checksum.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                bail!("invalid extra-data SHA-256 checksum: {checksum}");
            }
            Ok(ExtraDataSource {
                name,
                uri,
                size,
                checksum,
            })
        })
        .collect()
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        bail!("invalid extra-data filename: {name:?}");
    }
    Ok(())
}

fn checked_directory(path: &Path) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect directory {}", path.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        bail!("expected a non-symlink directory at {}", path.display());
    }
    Ok(path.to_path_buf())
}

fn download_source(source: &ExtraDataSource, work: &Path) -> Result<()> {
    let destination = work.join(&source.name);
    let partial = work.join(format!(".{}.part", source.name));
    println!("  Downloading extra data {}...", source.name);
    std::io::stdout().flush()?;
    let status = Command::new("fetch")
        .args(["-a", "-F", "-R", "-q", "-o"])
        .arg(&partial)
        .arg(&source.uri)
        .status()
        .with_context(|| format!("download extra data {}", source.name))?;
    if !status.success() {
        bail!(
            "download extra data {} failed with status {status}",
            source.name
        );
    }
    verify_download(&partial, source)?;
    fs::rename(&partial, &destination)
        .with_context(|| format!("publish extra data {}", destination.display()))?;
    Ok(())
}

fn verify_download(path: &Path, source: &ExtraDataSource) -> Result<()> {
    let actual_size = fs::metadata(path)
        .with_context(|| format!("inspect downloaded {}", source.name))?
        .len();
    if actual_size != source.size {
        bail!(
            "extra data {} has size {actual_size}, expected {}",
            source.name,
            source.size
        );
    }
    let mut checksum = Checksum::new(ChecksumType::Sha256).context("create SHA-256 checksum")?;
    let mut file = File::open(path).with_context(|| format!("open downloaded {}", source.name))?;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("read downloaded {}", source.name))?;
        if read == 0 {
            break;
        }
        checksum.update(&buffer[..read]);
    }
    let actual = checksum
        .string()
        .context("finalize extra-data SHA-256 checksum")?;
    if actual != source.checksum {
        bail!(
            "extra data {} has SHA-256 {actual}, expected {}",
            source.name,
            source.checksum
        );
    }
    Ok(())
}

fn remove_path_if_exists(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display()))
        }
        Ok(_) => fs::remove_file(path).with_context(|| format!("remove {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

#[cfg(test)]
#[path = "tests/extra_data.rs"]
mod tests;
