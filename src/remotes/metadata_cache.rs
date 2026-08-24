use super::ostree_summary::{
    parse_summary_collection_id, parse_summary_index, summary_digest_matches,
};
use super::ref_resolution::host_flatpak_arch;
use super::{trace_resolution, Remote};
use crate::installation::installation_paths::Installation;
use crate::ostree::Storage;
use anyhow::{bail, Context, Result};
use miniz_oxide::inflate::decompress_to_vec;
use std::fs;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub(super) fn decompress_gzip(data: &[u8]) -> Result<Vec<u8>> {
    let payload = gzip_deflate_payload(data)?;
    decompress_to_vec(payload).map_err(|error| anyhow::anyhow!("inflate gzip payload: {error:?}"))
}

fn gzip_deflate_payload(data: &[u8]) -> Result<&[u8]> {
    if data.len() < 18 || data[0] != 0x1f || data[1] != 0x8b {
        bail!("not a gzip stream");
    }
    if data[2] != 8 {
        bail!("unsupported gzip compression method {}", data[2]);
    }

    let flags = data[3];
    let mut offset = 10usize;
    if flags & 0x04 != 0 {
        if data.len() < offset + 2 {
            bail!("truncated gzip extra header");
        }
        let extra_len = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2 + extra_len;
    }
    if flags & 0x08 != 0 {
        offset = skip_gzip_c_string(data, offset).context("truncated gzip file name")?;
    }
    if flags & 0x10 != 0 {
        offset = skip_gzip_c_string(data, offset).context("truncated gzip comment")?;
    }
    if flags & 0x02 != 0 {
        offset += 2;
    }
    if data.len() < offset + 8 {
        bail!("truncated gzip payload");
    }

    Ok(&data[offset..data.len() - 8])
}

fn skip_gzip_c_string(data: &[u8], offset: usize) -> Option<usize> {
    data.get(offset..)?
        .iter()
        .position(|byte| *byte == 0)
        .map(|position| offset + position + 1)
}

fn safe_dir_fragment(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn load_arch_summary(
    paths: &Installation,
    remote: &Remote,
) -> Result<(String, PathBuf, Option<String>)> {
    let started = Instant::now();
    let arch = host_flatpak_arch()?;
    trace_resolution("detect architecture", started);
    let started = Instant::now();
    let index_path = fetch_summary_index(paths, remote);
    trace_resolution("fetch summary index", started);
    let started = Instant::now();
    let Ok(index_path) = index_path else {
        let summary_path = fetch_normal_summary(paths, remote)?;
        let collection_id = parse_summary_collection_id(&summary_path)?;
        trace_resolution("fetch normal summary", started);
        return Ok((arch, summary_path, collection_id));
    };
    let (digest, collection_id) = parse_summary_index(&index_path, &arch)?;
    if std::env::var_os("FREEBSD_FLATPAK_TRACE_RESOLUTION").is_some() {
        eprintln!("resolution metadata: {arch} summary {digest}");
    }
    trace_resolution("parse summary index", started);
    let started = Instant::now();
    let summary_path = fetch_subsummary(paths, remote, &arch, &digest)?;
    trace_resolution("fetch architecture summary", started);
    let started = Instant::now();
    let summary_path = cache_uncompressed_subsummary(paths, remote, &digest, &summary_path)?;
    trace_resolution("prepare architecture summary cache", started);
    Ok((arch, summary_path, collection_id))
}

fn fetch_summary_index(paths: &Installation, remote: &Remote) -> Result<PathBuf> {
    let remote_dir = paths.remote_metadata(&remote.name);
    let path = remote_dir.join("summary.idx");
    let signature_path = remote_dir.join("summary.idx.sig");
    let checked = remote_dir.join("summary.idx.checked");
    fs::create_dir_all(&remote_dir).context("create remote metadata directory")?;
    if (!remote.gpg_verify || signature_path.is_file()) && metadata_is_fresh(&path, &checked)? {
        return Ok(path);
    }
    let _lock = MetadataLock::acquire(&remote_dir.join("summary.idx.lock"))?;
    if (!remote.gpg_verify || signature_path.is_file()) && metadata_is_fresh(&path, &checked)? {
        return Ok(path);
    }
    let candidate = remote_dir.join("summary.idx.candidate");
    let signature_candidate = remote_dir.join("summary.idx.sig.candidate");
    let refresh = || -> Result<()> {
        fetch_metadata_file(
            &format!("{}/summary.idx", remote.url),
            &candidate,
            &format!("{} metadata index", remote.name),
        )?;
        if remote.gpg_verify {
            fetch_metadata_file(
                &format!("{}/summary.idx.sig", remote.url),
                &signature_candidate,
                &format!("{} metadata signature", remote.name),
            )?;
            let summary =
                fs::read(&candidate).with_context(|| format!("read {}", candidate.display()))?;
            let signatures = fs::read(&signature_candidate)
                .with_context(|| format!("read {}", signature_candidate.display()))?;
            Storage::open(paths)?.verify_summary(&remote.name, &summary, &signatures)?;
        }
        fs::rename(&candidate, &path).with_context(|| format!("activate {}", path.display()))?;
        if remote.gpg_verify {
            fs::rename(&signature_candidate, &signature_path)
                .with_context(|| format!("activate {}", signature_path.display()))?;
        }
        Ok(())
    };
    match refresh() {
        Ok(()) => {
            fs::write(&checked, format!("{}\n", unix_timestamp()))
                .with_context(|| format!("write {}", checked.display()))?;
        }
        Err(error) if path.is_file() => {
            eprintln!(
                "warning: refresh {} metadata index failed; using cache: {error:#}",
                remote.name
            );
        }
        Err(error) => return Err(error),
    }
    Ok(path)
}

fn fetch_subsummary(
    paths: &Installation,
    remote: &Remote,
    arch: &str,
    digest: &str,
) -> Result<PathBuf> {
    let summaries = paths.remote_metadata(&remote.name).join("summaries");
    fs::create_dir_all(&summaries).context("create architecture summary cache")?;
    let path = summaries.join(format!("{digest}.gz"));
    if path.is_file() {
        return Ok(path);
    }
    let _lock = MetadataLock::acquire(&summaries.join(format!("{digest}.lock")))?;
    if path.is_file() {
        return Ok(path);
    }
    fetch_metadata_file(
        &format!("{}/summaries/{digest}.gz", remote.url),
        &path,
        &format!("{} metadata for {arch}", remote.name),
    )?;
    Ok(path)
}

fn cache_uncompressed_subsummary(
    paths: &Installation,
    remote: &Remote,
    digest: &str,
    compressed_path: &Path,
) -> Result<PathBuf> {
    let path = paths
        .remote_metadata(&remote.name)
        .join("summaries")
        .join(format!("{digest}.sub"));
    if path.is_file() {
        let data = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        if summary_digest_matches(&data, digest)? {
            return Ok(path);
        }
        fs::remove_file(&path).with_context(|| format!("remove invalid {}", path.display()))?;
    }
    let _lock = MetadataLock::acquire(&path.with_extension("sub.lock"))?;
    if path.is_file() {
        let data = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        if summary_digest_matches(&data, digest)? {
            return Ok(path);
        }
        fs::remove_file(&path).with_context(|| format!("remove invalid {}", path.display()))?;
    }
    let compressed =
        fs::read(compressed_path).with_context(|| format!("read {}", compressed_path.display()))?;
    let data = decompress_gzip(&compressed)
        .with_context(|| format!("decompress {}", compressed_path.display()))?;
    if !summary_digest_matches(&data, digest)? {
        let _ = fs::remove_file(compressed_path);
        bail!(
            "downloaded {} architecture summary failed SHA-256 verification",
            remote.name
        );
    }
    let partial = path.with_extension("sub.part");
    fs::write(&partial, data).with_context(|| format!("write {}", partial.display()))?;
    fs::rename(&partial, &path)
        .with_context(|| format!("complete summary cache {}", path.display()))?;
    Ok(path)
}

fn fetch_metadata_file(url: &str, path: &Path, label: &str) -> Result<()> {
    let partial = path.with_file_name(format!(
        ".{}.part",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("metadata")
    ));
    println!("  Downloading {label}...");
    let _ = std::io::stdout().flush();

    let mut command = Command::new("fetch");
    command
        .arg("-a")
        .arg("-F")
        .arg("-r")
        .arg("-R")
        .arg("-q")
        .arg("-o")
        .arg(&partial);
    let mut child = command
        .arg(url)
        .spawn()
        .with_context(|| format!("download {label}"))?;
    let terminal = std::io::stdout().is_terminal();
    let mut last_bytes = None;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("wait for {label} download"))?
        {
            break status;
        }
        if terminal {
            let bytes = fs::metadata(&partial)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            if last_bytes != Some(bytes) {
                print!("\r\x1b[2K    Received {}", format_byte_count(bytes));
                let _ = std::io::stdout().flush();
                last_bytes = Some(bytes);
            }
        }
        std::thread::sleep(Duration::from_millis(150));
    };
    let bytes = fs::metadata(&partial)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if terminal {
        print!("\r\x1b[2K");
    }
    if !status.success() {
        bail!("download {label} failed with status {status}; partial download kept for retry");
    }
    fs::rename(&partial, path)
        .with_context(|| format!("complete metadata download {}", path.display()))?;
    println!("  Downloaded {label} ({})", format_byte_count(bytes));
    let _ = std::io::stdout().flush();
    Ok(())
}

fn format_byte_count(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

struct MetadataLock {
    path: PathBuf,
}

impl MetadataLock {
    fn acquire(path: &Path) -> Result<Self> {
        let started = std::time::Instant::now();
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id())?;
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if stale_metadata_lock(path) {
                        let _ = fs::remove_file(path);
                        continue;
                    }
                    if started.elapsed() > std::time::Duration::from_secs(300) {
                        bail!(
                            "timed out waiting for metadata refresh lock {}",
                            path.display()
                        );
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("create {}", path.display()))
                }
            }
        }
    }
}

impl Drop for MetadataLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn stale_metadata_lock(path: &Path) -> bool {
    let Ok(pid) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(pid) = pid.trim().parse::<i32>() else {
        return false;
    };
    unsafe {
        libc::kill(pid, 0) != 0
            && std::io::Error::last_os_error().raw_os_error() != Some(libc::EPERM)
    }
}

fn metadata_is_fresh(path: &Path, checked: &Path) -> Result<bool> {
    if !path.is_file() || !checked.is_file() {
        return Ok(false);
    }
    let ttl = std::env::var("FREEBSD_FLATPAK_METADATA_TTL")
        .ok()
        .map(|value| {
            value
                .parse::<u64>()
                .context("parse FREEBSD_FLATPAK_METADATA_TTL")
        })
        .transpose()?
        .unwrap_or(300);
    let Ok(timestamp) = fs::read_to_string(checked)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .ok_or(())
    else {
        return Ok(false);
    };
    Ok(unix_timestamp().saturating_sub(timestamp) < ttl)
}

fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub(super) fn fetch_appstream(remote: &Remote, remote_dir: &Path, arch: &str) -> Result<PathBuf> {
    let path = remote_dir.join(format!("appstream-{}.xml.gz", safe_dir_fragment(arch)));
    let checked = remote_dir.join(format!("appstream-{}.checked", safe_dir_fragment(arch)));
    fs::create_dir_all(remote_dir).context("create remote metadata directory")?;
    if metadata_is_fresh(&path, &checked)? {
        return Ok(path);
    }
    let _lock = MetadataLock::acquire(
        &remote_dir.join(format!("appstream-{}.lock", safe_dir_fragment(arch))),
    )?;
    if metadata_is_fresh(&path, &checked)? {
        return Ok(path);
    }
    match fetch_metadata_file(
        &format!("{}/appstream/{arch}/appstream.xml.gz", remote.url),
        &path,
        &format!("{} AppStream metadata", remote.name),
    ) {
        Ok(()) => {
            fs::write(&checked, format!("{}\n", unix_timestamp()))
                .with_context(|| format!("write {}", checked.display()))?;
        }
        Err(error) if path.is_file() => {
            eprintln!(
                "warning: refresh {} app replacement metadata failed; using cache: {error:#}",
                remote.name
            );
        }
        Err(error) => return Err(error),
    }
    Ok(path)
}

fn fetch_normal_summary(paths: &Installation, remote: &Remote) -> Result<PathBuf> {
    let remote_dir = paths.remote_metadata(&remote.name);
    fs::create_dir_all(&remote_dir)?;
    let path = remote_dir.join("summary");
    let checked = remote_dir.join("summary.checked");
    if metadata_is_fresh(&path, &checked)? {
        return Ok(path);
    }
    let _lock = MetadataLock::acquire(&remote_dir.join("summary.lock"))?;
    if metadata_is_fresh(&path, &checked)? {
        return Ok(path);
    }
    let candidate = remote_dir.join("summary.candidate");
    let signature_candidate = remote_dir.join("summary.sig.candidate");
    let refresh = || -> Result<()> {
        fetch_metadata_file(
            &format!("{}/summary", remote.url),
            &candidate,
            &format!("{} summary", remote.name),
        )?;
        if remote.gpg_verify {
            fetch_metadata_file(
                &format!("{}/summary.sig", remote.url),
                &signature_candidate,
                &format!("{} summary signature", remote.name),
            )?;
            let summary = fs::read(&candidate)?;
            let signature = fs::read(&signature_candidate)?;
            Storage::open(paths)?.verify_summary(&remote.name, &summary, &signature)?;
        }
        fs::rename(&candidate, &path)?;
        if remote.gpg_verify {
            fs::rename(&signature_candidate, remote_dir.join("summary.sig"))?;
        }
        Ok(())
    };
    match refresh() {
        Ok(()) => fs::write(&checked, format!("{}\n", unix_timestamp()))?,
        Err(error) if path.is_file() => eprintln!(
            "warning: refresh {} summary failed; using cache: {error:#}",
            remote.name
        ),
        Err(error) => return Err(error),
    }
    Ok(path)
}
