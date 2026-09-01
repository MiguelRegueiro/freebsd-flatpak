use super::list_table::{self, InstalledRow, Options};
use crate::installation::{self as state, installation_paths::Installation};
use anyhow::{bail, Result};
use std::fs;
use std::path::Path;

pub(crate) fn cmd_list(paths: &Installation, args: Vec<String>) -> Result<()> {
    let options = list_table::parse_options(&args)?;
    if options.columns_help {
        list_table::print_column_help();
        return Ok(());
    }
    let rows = installed_rows(paths, &options)?;
    print!(
        "{}",
        list_table::render(&rows, &options, super::style::stdout_enabled())
    );
    Ok(())
}

fn installed_rows(paths: &Installation, options: &Options) -> Result<Vec<InstalledRow>> {
    let mut rows = Vec::new();
    if options.apps {
        for app in state::list_apps(paths)? {
            let (name, version) =
                installed_appstream_fields(&state::absolute(paths, &app.app_dir), &app.app_id);
            rows.push(InstalledRow {
                name,
                application: app.app_id,
                arch: app.arch,
                version,
                branch: app.branch,
                runtime: app.runtime_ref,
                ref_name: app.app_ref,
                origin: app.origin,
                active: app.app_commit,
                installed_size: app.installed_size,
            });
        }
    }
    if options.runtimes {
        for runtime in state::list_runtimes(paths)? {
            let (application, arch, branch) = split_ref(&runtime.runtime_ref)?;
            let (name, version) = installed_appstream_fields(
                &state::absolute(paths, &runtime.runtime_dir),
                &application,
            );
            rows.push(InstalledRow {
                name,
                application,
                arch,
                version,
                branch,
                runtime: String::new(),
                ref_name: format!("runtime/{}", runtime.runtime_ref),
                origin: runtime.origin,
                active: runtime.runtime_commit,
                installed_size: runtime.installed_size,
            });
        }
    }
    rows.sort_by(|left, right| left.ref_name.cmp(&right.ref_name));
    rows.dedup_by(|left, right| {
        left.ref_name == right.ref_name
            && left.origin == right.origin
            && left.active == right.active
    });
    Ok(rows)
}

fn installed_appstream_fields(checkout: &Path, app_id: &str) -> (String, String) {
    for relative in ["files/share/metainfo", "files/share/appdata"] {
        let directory = checkout.join(relative);
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        let mut paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "xml"))
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let Ok(xml) = fs::read_to_string(path) else {
                continue;
            };
            if let Some(info) = crate::remotes::parse_appstream_info(&xml, app_id) {
                return (
                    info.name.unwrap_or_default(),
                    info.version.unwrap_or_default(),
                );
            }
        }
    }
    (String::new(), String::new())
}

fn split_ref(value: &str) -> Result<(String, String, String)> {
    let mut parts = value.splitn(3, '/');
    let application = parts.next().unwrap_or_default();
    let arch = parts.next().unwrap_or_default();
    let branch = parts.next().unwrap_or_default();
    if application.is_empty() || arch.is_empty() || branch.is_empty() {
        bail!("invalid installed ref: {value}");
    }
    Ok((
        application.to_string(),
        arch.to_string(),
        branch.to_string(),
    ))
}

#[cfg(test)]
#[path = "tests/list.rs"]
mod tests;
