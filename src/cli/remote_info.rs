use crate::{paths::Installation, remote, storage};
use anyhow::{bail, Context, Result};
use std::fmt::Write as _;
use std::io::IsTerminal;

pub(crate) fn cmd_remote_info(paths: &Installation, args: Vec<String>) -> Result<()> {
    let options = parse_remote_info_args(args)?;
    let metadata = remote::load_remote_metadata(paths)?;
    let remote = metadata.resolve_app(&options.app_id, true)?;
    let appstream = match metadata.appstream_info(&remote.app_id) {
        Ok(info) => info,
        Err(error) => {
            eprintln!("warning: load Flathub AppStream metadata: {error:#}");
            None
        }
    };
    let styled = std::io::stdout().is_terminal();

    if options.log {
        let (_, history) = metadata.app_history(paths, &options.app_id)?;
        print!(
            "{}",
            remote_log_output(
                &remote,
                &history,
                appstream.as_ref(),
                metadata.collection_id(),
                styled,
            )
        );
    } else if let Some(commit) = options.commit {
        let current_commit = remote.app_commit.clone();
        let (remote, commit) = metadata.app_commit(paths, &remote.app_ref, &commit)?;
        let historical = commit.checksum != current_commit;
        print!(
            "{}",
            remote_info_output(
                &remote,
                Some(&commit),
                appstream.as_ref(),
                metadata.collection_id(),
                historical,
                styled,
            )
        );
    } else {
        print!(
            "{}",
            remote_info_output(
                &remote,
                None,
                appstream.as_ref(),
                metadata.collection_id(),
                false,
                styled,
            )
        );
    }
    Ok(())
}

pub(super) fn remote_log_output(
    remote: &remote::RemoteApp,
    history: &[storage::CommitInfo],
    appstream: Option<&remote::AppstreamInfo>,
    remote_collection: Option<&str>,
    styled: bool,
) -> String {
    let current = history.first();
    let version = current
        .and_then(|commit| commit.version.as_deref())
        .or_else(|| appstream.and_then(|info| info.version.as_deref()));
    let collection = current
        .and_then(|commit| commit.collection_id.as_deref())
        .or(remote_collection);
    let mut output = remote_metadata_output(
        remote,
        appstream,
        version,
        appstream.and_then(|info| info.license.as_deref()),
        collection,
        styled,
    );
    if let Some(current) = history.first() {
        output.push('\n');
        append_commit(&mut output, current, true, styled);
    }
    if history.len() > 1 {
        append_label(&mut output, "History:", None, styled);
        for commit in &history[1..] {
            output.push('\n');
            append_commit(&mut output, commit, false, styled);
        }
    }
    output
}

pub(super) fn remote_info_output(
    remote: &remote::RemoteApp,
    commit: Option<&storage::CommitInfo>,
    appstream: Option<&remote::AppstreamInfo>,
    remote_collection: Option<&str>,
    historical: bool,
    styled: bool,
) -> String {
    let version = commit
        .and_then(|commit| commit.version.as_deref())
        .or_else(|| {
            (!historical)
                .then(|| appstream.and_then(|info| info.version.as_deref()))
                .flatten()
        });
    let license = (!historical)
        .then(|| appstream.and_then(|info| info.license.as_deref()))
        .flatten();
    let collection = commit
        .and_then(|commit| commit.collection_id.as_deref())
        .or_else(|| (!historical).then_some(remote_collection).flatten());
    let mut output =
        remote_metadata_output(remote, appstream, version, license, collection, styled);
    output.push('\n');
    if let Some(commit) = commit {
        append_commit(&mut output, commit, true, styled);
    } else {
        append_label(&mut output, "Commit:", Some(&remote.app_commit), styled);
    }
    output
}

fn remote_metadata_output(
    remote: &remote::RemoteApp,
    appstream: Option<&remote::AppstreamInfo>,
    version: Option<&str>,
    license: Option<&str>,
    collection: Option<&str>,
    styled: bool,
) -> String {
    let mut output = String::from("\n");
    if let Some(name) = appstream.and_then(|info| info.name.as_deref()) {
        output.push_str(name);
        if let Some(summary) = appstream.and_then(|info| info.summary.as_deref()) {
            output.push_str(" - ");
            output.push_str(summary);
        }
        output.push_str("\n\n");
    }
    append_label(&mut output, "ID:", Some(&remote.app_id), styled);
    append_label(&mut output, "Ref:", Some(&remote.app_ref), styled);
    append_label(&mut output, "Arch:", Some(&remote.arch), styled);
    append_label(&mut output, "Branch:", Some(&remote.branch), styled);
    if let Some(version) = version {
        append_label(&mut output, "Version:", Some(version), styled);
    }
    if let Some(license) = license {
        append_label(&mut output, "License:", Some(license), styled);
    }
    if let Some(collection) = collection {
        append_label(&mut output, "Collection:", Some(collection), styled);
    }
    if let Some(size) = remote.download_size {
        append_label(
            &mut output,
            "Download Size:",
            Some(&format_remote_size(size)),
            styled,
        );
    }
    if let Some(size) = remote.installed_size {
        append_label(
            &mut output,
            "Installed Size:",
            Some(&format_remote_size(size)),
            styled,
        );
    }
    append_label(&mut output, "Runtime:", Some(&remote.runtime_ref), styled);
    if let Some(sdk) = &remote.sdk_ref {
        append_label(&mut output, "Sdk:", Some(sdk), styled);
    }
    output
}

fn append_commit(
    output: &mut String,
    commit: &storage::CommitInfo,
    include_parent: bool,
    styled: bool,
) {
    append_label(output, "Commit:", Some(&commit.checksum), styled);
    if include_parent {
        if let Some(parent) = &commit.parent {
            append_label(output, "Parent:", Some(parent), styled);
        }
    }
    if !commit.subject.is_empty() {
        append_label(output, "Subject:", Some(&commit.subject), styled);
    }
    if let Ok(date) = glib::DateTime::from_unix_utc(commit.timestamp as i64)
        .and_then(|date| date.format("%Y-%m-%d %H:%M:%S +0000"))
    {
        append_label(output, "Date:", Some(date.as_str()), styled);
    }
}

fn append_label(output: &mut String, label: &str, value: Option<&str>, styled: bool) {
    const WIDTH: usize = 15;
    let padding = WIDTH.saturating_sub(label.len());
    output.push_str(&" ".repeat(padding));
    if styled {
        let _ = write!(output, "\x1b[1m{label}\x1b[0m");
    } else {
        output.push_str(label);
    }
    if let Some(value) = value {
        output.push(' ');
        output.push_str(value);
    }
    output.push('\n');
}

fn format_remote_size(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.1} kB", bytes as f64 / 1_000.0)
    } else {
        format!("{bytes} bytes")
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct RemoteInfoOptions {
    pub(super) log: bool,
    pub(super) commit: Option<String>,
    pub(super) app_id: String,
}

pub(super) fn parse_remote_info_args(args: Vec<String>) -> Result<RemoteInfoOptions> {
    let mut log = false;
    let mut commit = None;
    let mut operands = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--log" => log = true,
            "--commit" => {
                if commit.is_some() {
                    bail!("--commit may only be specified once");
                }
                commit = Some(args.next().context("missing value for --commit")?);
            }
            _ if arg.starts_with("--commit=") => {
                if commit.is_some() {
                    bail!("--commit may only be specified once");
                }
                commit = Some(value_after_equals(&arg).to_string());
            }
            _ if arg.starts_with('-') => bail!("unknown remote-info option: {arg}"),
            _ => operands.push(arg),
        }
    }
    if log && commit.is_some() {
        bail!("--log and --commit cannot be used together");
    }
    if operands.len() != 2 {
        bail!("usage: flatpak remote-info [--log | --commit=COMMIT] flathub <app-id>");
    }
    if operands[0] != "flathub" {
        bail!("remote is not configured: {}", operands[0]);
    }
    Ok(RemoteInfoOptions {
        log,
        commit,
        app_id: operands.remove(1),
    })
}

fn value_after_equals(arg: &str) -> &str {
    arg.split_once('=').map(|(_, value)| value).unwrap_or("")
}

#[cfg(test)]
#[path = "tests/remote_info.rs"]
mod tests;
