use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub(super) fn rewrite_desktop_file(
    source: &Path,
    target: &Path,
    flatpak_bin: &Path,
    app_id: &str,
) -> Result<()> {
    let data = fs::read_to_string(source)
        .with_context(|| format!("read desktop file {}", source.display()))?;
    let mut rewritten = String::with_capacity(data.len() + 128);

    for line in data.lines() {
        if let Some(exec) = line.strip_prefix("Exec=") {
            rewritten.push_str("Exec=");
            rewritten.push_str(&desktop_exec(flatpak_bin, app_id, exec));
        } else if line == "DBusActivatable=true" {
            rewritten.push_str("DBusActivatable=false");
        } else if line.starts_with("TryExec=") {
            rewritten.push_str("TryExec=");
            rewritten.push_str(&desktop_quote_arg(&flatpak_bin.display().to_string()));
        } else {
            rewritten.push_str(line);
        }
        rewritten.push('\n');
    }

    fs::write(target, rewritten).with_context(|| format!("write {}", target.display()))
}

pub(super) fn desktop_exec(flatpak_bin: &Path, app_id: &str, original: &str) -> String {
    let tail = exec_tail_after_command(original);
    let mut exec = format!(
        "{} run {}",
        desktop_quote_arg(&flatpak_bin.display().to_string()),
        desktop_quote_arg(app_id)
    );
    if !tail.is_empty() {
        exec.push_str(" -- ");
        exec.push_str(tail);
    }
    exec
}

pub(super) fn exec_tail_after_command(original: &str) -> &str {
    let trimmed = original.trim_start();
    if trimmed.is_empty() {
        return "";
    }

    let mut chars = trimmed.char_indices();
    let Some((_, first)) = chars.next() else {
        return "";
    };

    if first == '"' || first == '\'' {
        let quote = first;
        let mut escaped = false;
        for (idx, ch) in chars {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == quote {
                return trimmed[idx + ch.len_utf8()..].trim_start();
            }
        }
        return "";
    }

    let mut escaped = false;
    for (idx, ch) in trimmed.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch.is_whitespace() {
            return trimmed[idx..].trim_start();
        }
    }

    ""
}

fn desktop_quote_arg(arg: &str) -> String {
    if arg
        .chars()
        .all(|ch| !ch.is_whitespace() && !matches!(ch, '"' | '\\' | '$' | '`'))
    {
        return arg.to_string();
    }

    let mut quoted = String::from("\"");
    for ch in arg.chars() {
        if matches!(ch, '"' | '\\' | '$' | '`') {
            quoted.push('\\');
        }
        quoted.push(ch);
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
#[path = "tests/desktop_entries.rs"]
mod tests;
