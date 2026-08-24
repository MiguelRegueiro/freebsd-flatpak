use super::list_table::{self, InstalledRow, Options};
use crate::installation::{self as state, installation_paths::Installation};
use anyhow::{bail, Result};

pub(crate) fn cmd_list(paths: &Installation, args: Vec<String>) -> Result<()> {
    let options = list_table::parse_options(&args)?;
    if options.columns_help {
        list_table::print_column_help();
        return Ok(());
    }
    let rows = installed_rows(paths, &options)?;
    print!("{}", list_table::render(&rows, &options));
    Ok(())
}

fn installed_rows(paths: &Installation, options: &Options) -> Result<Vec<InstalledRow>> {
    let mut rows = Vec::new();
    if options.apps {
        for app in state::list_apps(paths)? {
            rows.push(InstalledRow {
                application: app.app_id,
                arch: app.arch,
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
            rows.push(InstalledRow {
                application,
                arch,
                branch,
                runtime: String::new(),
                ref_name: format!("runtime/{}", runtime.runtime_ref),
                origin: runtime.origin,
                active: runtime.runtime_commit,
                installed_size: runtime.installed_size,
            });
        }
        for extension in state::list_extensions(paths)? {
            let partial = extension
                .ref_name
                .strip_prefix("runtime/")
                .unwrap_or(&extension.ref_name);
            let (application, arch, branch) = split_ref(partial)?;
            rows.push(InstalledRow {
                application,
                arch,
                branch,
                runtime: String::new(),
                ref_name: extension.ref_name,
                origin: extension.origin,
                active: extension.commit,
                installed_size: extension.installed_size,
            });
        }
    }
    rows.sort_by(|left, right| left.ref_name.cmp(&right.ref_name));
    Ok(rows)
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
