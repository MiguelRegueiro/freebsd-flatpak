use crate::flatpak_metadata::{section_entries, value};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

const LIST_KEYS: &[(&str, &str)] = &[
    ("Context", "shared"),
    ("Context", "sockets"),
    ("Context", "devices"),
    ("Context", "features"),
    ("Context", "filesystems"),
    ("Context", "persistent"),
    ("Context", "unset-environment"),
];

/// Loads application metadata with Flatpak's user-wide and per-application
/// static overrides applied in precedence order.
pub(crate) fn effective_metadata(
    metadata_path: &Path,
    overrides_dir: &Path,
    app_id: &str,
) -> Result<String> {
    let metadata = fs::read_to_string(metadata_path)
        .with_context(|| format!("read Flatpak metadata {}", metadata_path.display()))?;
    effective_metadata_from_sources(
        &metadata,
        read_optional(&overrides_dir.join("global"))?.as_deref(),
        read_optional(&overrides_dir.join(app_id))?.as_deref(),
    )
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(data) => Ok(Some(data)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("read Flatpak override {}", path.display()))
        }
    }
}

fn effective_metadata_from_sources(
    metadata: &str,
    global: Option<&str>,
    application: Option<&str>,
) -> Result<String> {
    let mut key_file = parse_key_file(metadata);
    for overlay in global.into_iter().chain(application) {
        apply_overlay(&mut key_file, overlay);
    }
    Ok(render_key_file(&key_file))
}

type KeyFile = BTreeMap<String, BTreeMap<String, String>>;

fn parse_key_file(data: &str) -> KeyFile {
    let mut result = KeyFile::new();
    for section in crate::flatpak_metadata::section_names(data) {
        let entries = result.entry(section.clone()).or_default();
        for (key, value) in section_entries(data, &section) {
            entries.insert(key, value);
        }
    }
    result
}

fn apply_overlay(base: &mut KeyFile, overlay: &str) {
    for section in crate::flatpak_metadata::section_names(overlay) {
        for (key, overlay_value) in section_entries(overlay, &section) {
            let value = if LIST_KEYS.contains(&(section.as_str(), key.as_str())) {
                merge_list(
                    base.get(&section).and_then(|entries| entries.get(&key)),
                    &overlay_value,
                    key == "filesystems",
                )
            } else {
                overlay_value
            };
            base.entry(section.clone()).or_default().insert(key, value);
        }
    }
}

fn merge_list(base: Option<&String>, overlay: &str, filesystem: bool) -> String {
    let mut values = split_list(base.map(String::as_str).unwrap_or_default());
    for item in split_list(overlay) {
        let identity = list_identity(&item, filesystem);
        values.retain(|existing| list_identity(existing, filesystem) != identity);
        values.push(item);
    }
    if values.is_empty() {
        String::new()
    } else {
        format!("{};", values.join(";"))
    }
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn list_identity(item: &str, filesystem: bool) -> &str {
    let item = item.strip_prefix('!').unwrap_or(item);
    if !filesystem {
        return item.split_once('@').map(|(name, _)| name).unwrap_or(item);
    }
    match item.rsplit_once(':') {
        Some((path, "ro" | "rw" | "create")) => path,
        _ => item,
    }
}

fn render_key_file(key_file: &KeyFile) -> String {
    let mut rendered = String::new();
    for (section, entries) in key_file {
        rendered.push_str(&format!("[{section}]\n"));
        for (key, value) in entries {
            rendered.push_str(&format!("{key}={value}\n"));
        }
        rendered.push('\n');
    }
    rendered
}

pub(super) fn permission_enabled(metadata: &str, section: &str, key: &str, target: &str) -> bool {
    value(metadata, section, key)
        .map(|items| {
            split_list(&items)
                .into_iter()
                .rev()
                .find(|item| list_identity(item, key == "filesystems") == target)
                .is_some_and(|item| !item.starts_with('!'))
        })
        .unwrap_or(false)
}

#[cfg(test)]
#[path = "tests/static_overrides.rs"]
mod tests;
