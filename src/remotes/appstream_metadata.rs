use super::metadata_cache::{decompress_gzip, fetch_appstream};
use super::{AppstreamInfo, RemoteMetadata};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

impl RemoteMetadata {
    pub fn appstream_info(&self, app_id: &str) -> Result<Option<AppstreamInfo>> {
        let path = fetch_appstream(&self.remote, &self.remote_dir, &self.arch)?;
        let compressed = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let xml = decompress_appstream_xml(&compressed)
            .with_context(|| format!("decompress {}", path.display()))?;
        Ok(parse_appstream_info(&xml, app_id))
    }
}

pub(super) fn fetch_appstream_replacements(
    remote: &crate::remotes::Remote,
    remote_dir: &Path,
    arch: &str,
) -> Result<BTreeMap<String, Vec<String>>> {
    let path = fetch_appstream(remote, remote_dir, arch)?;
    let compressed = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let xml = decompress_appstream_xml(&compressed)
        .with_context(|| format!("decompress {}", path.display()))?;
    Ok(parse_appstream_replacements(&xml))
}

pub(super) fn decompress_appstream_xml(data: &[u8]) -> Result<String> {
    String::from_utf8(decompress_gzip(data)?).context("AppStream XML is not UTF-8")
}

fn find_component_start(xml: &str) -> Option<usize> {
    let mut offset = 0usize;
    while let Some(relative_start) = xml[offset..].find("<component") {
        let start = offset + relative_start;
        let boundary = xml.as_bytes().get(start + "<component".len()).copied()?;
        if boundary == b'>' || boundary.is_ascii_whitespace() {
            return Some(start);
        }
        offset = start + "<component".len();
    }
    None
}

pub(super) fn parse_appstream_replacements(xml: &str) -> BTreeMap<String, Vec<String>> {
    let mut replacements: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut rest = xml;

    while let Some(start) = find_component_start(rest) {
        rest = &rest[start..];
        let Some(tag_end) = rest.find('>') else {
            break;
        };
        let component_body = &rest[tag_end + 1..];
        let Some(end) = component_body.find("</component>") else {
            break;
        };
        let component = &component_body[..end];
        rest = &component_body[end + "</component>".len()..];

        let Some(new_id) = first_direct_xml_text(component, "id") else {
            continue;
        };
        let mut component_rest = component;
        while let Some(replaces_start) = component_rest.find("<replaces>") {
            component_rest = &component_rest[replaces_start + "<replaces>".len()..];
            let Some(replaces_end) = component_rest.find("</replaces>") else {
                break;
            };
            let replaces = &component_rest[..replaces_end];
            component_rest = &component_rest[replaces_end + "</replaces>".len()..];

            for old_id in xml_texts(replaces, "id") {
                replacements.entry(old_id).or_default().push(new_id.clone());
            }
        }
    }

    replacements
}

pub(crate) fn parse_appstream_info(xml: &str, app_id: &str) -> Option<AppstreamInfo> {
    let mut rest = xml;
    while let Some(start) = find_component_start(rest) {
        rest = &rest[start..];
        let tag_end = rest.find('>')?;
        let component_body = &rest[tag_end + 1..];
        let end = component_body.find("</component>")?;
        let component = &component_body[..end];
        rest = &component_body[end + "</component>".len()..];
        let matches_app = first_direct_xml_text(component, "id")
            .is_some_and(|id| id == app_id || id.strip_suffix(".desktop") == Some(app_id));
        if !matches_app {
            continue;
        }

        let version = first_release_version(component);
        return Some(AppstreamInfo {
            name: first_direct_xml_text(component, "name"),
            summary: first_direct_xml_text(component, "summary"),
            version,
            license: first_direct_xml_text(component, "project_license"),
        });
    }
    None
}

fn first_release_version(component: &str) -> Option<String> {
    let mut rest = component;
    while let Some(start) = rest.find("<release") {
        rest = &rest[start..];
        let end = rest.find('>')?;
        let tag = &rest[..=end];
        if let Some(version) = xml_attribute(tag, "version") {
            return Some(version);
        }
        rest = &rest[end + 1..];
    }
    None
}

fn xml_attribute(tag: &str, attribute: &str) -> Option<String> {
    let needle = format!("{attribute}=\"");
    let value = &tag[tag.find(&needle)? + needle.len()..];
    let end = value.find('"')?;
    let value = value[..end].trim();
    (!value.is_empty()).then(|| xml_unescape_text(value))
}

fn first_direct_xml_text(xml: &str, tag: &str) -> Option<String> {
    let mut depth = 0usize;
    let mut offset = 0usize;
    while let Some(relative_start) = xml[offset..].find('<') {
        let start = offset + relative_start;
        if xml[start..].starts_with("<!--") {
            offset = start + xml[start..].find("-->")? + 3;
            continue;
        }
        let end = start + xml[start..].find('>')?;
        let markup = xml[start + 1..end].trim();
        if markup.starts_with('?') || markup.starts_with('!') {
            offset = end + 1;
            continue;
        }
        if markup.starts_with('/') {
            depth = depth.saturating_sub(1);
            offset = end + 1;
            continue;
        }

        let self_closing = markup.ends_with('/');
        let element = markup
            .trim_end_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or_default();
        if depth == 0 && element == tag && !markup.contains("xml:lang=") {
            let value_start = end + 1;
            let close = format!("</{tag}>");
            let value_end = value_start + xml[value_start..].find(&close)?;
            let value = xml[value_start..value_end].trim();
            if !value.is_empty() {
                return Some(xml_unescape_text(value));
            }
        }
        if !self_closing {
            depth += 1;
        }
        offset = end + 1;
    }
    None
}

fn xml_texts(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut values = Vec::new();
    let mut rest = xml;

    while let Some(start) = rest.find(&open) {
        let value_start = start + open.len();
        let after_open = &rest[value_start..];
        let Some(end) = after_open.find(&close) else {
            break;
        };
        let value = after_open[..end].trim();
        if !value.is_empty() {
            values.push(xml_unescape_text(value));
        }
        rest = &after_open[end + close.len()..];
    }

    values
}

fn xml_unescape_text(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
#[path = "tests/appstream_metadata.rs"]
mod tests;
