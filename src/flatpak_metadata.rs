pub(crate) fn value(metadata: &str, section: &str, key: &str) -> Option<String> {
    let mut current = "";
    for line in metadata.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            current = &line[1..line.len() - 1];
            continue;
        }
        if current == section {
            let Some((candidate, value)) = line.split_once('=') else {
                continue;
            };
            if candidate.trim() == key {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

pub(crate) fn section_entries(metadata: &str, section: &str) -> Vec<(String, String)> {
    let mut current = "";
    let mut entries = Vec::new();
    for line in metadata.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            current = &line[1..line.len() - 1];
            continue;
        }
        if current != section {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !key.is_empty() {
            entries.push((key.to_string(), value.trim().to_string()));
        }
    }
    entries
}

pub(crate) fn has_section(metadata: &str, section: &str) -> bool {
    metadata.lines().any(|line| {
        let line = line.trim();
        line.starts_with('[') && line.ends_with(']') && &line[1..line.len() - 1] == section
    })
}

pub(crate) fn sections_with_prefix(metadata: &str, prefix: &str) -> Vec<String> {
    metadata
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('[') && line.ends_with(']') {
                let section = &line[1..line.len() - 1];
                if section.starts_with(prefix) {
                    return Some(section.to_string());
                }
            }
            None
        })
        .collect()
}
