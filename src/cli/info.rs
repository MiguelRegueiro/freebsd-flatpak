use crate::{
    flatpak_ref::{FlatpakRef, PartialRef},
    installation::{self as state, installation_paths::Installation},
};
use anyhow::{bail, Context, Result};
use std::path::PathBuf;

struct InstalledInfo {
    id: String,
    ref_name: String,
    arch: String,
    branch: String,
    origin: String,
    commit: String,
    installed_size: u64,
    location: PathBuf,
    runtime: Option<String>,
}

pub(crate) fn cmd_info(paths: &Installation, args: Vec<String>) -> Result<()> {
    let mut show_size = false;
    let mut show_location = false;
    let mut operands = Vec::new();
    for arg in args {
        match arg.as_str() {
            "-s" | "--show-size" => show_size = true,
            "-l" | "--show-location" => show_location = true,
            _ if arg.starts_with('-') => bail!("unknown info option: {arg}"),
            _ => operands.push(arg),
        }
    }
    let name = operands.first().context("NAME must be specified")?;
    let branch = operands.get(1).map(String::as_str);
    if operands.len() > 2 {
        bail!("too many arguments for info");
    }
    crate::flatpak_ref::validate_partial_ref(name, branch)?;
    let info = resolve_installed(paths, name, branch)?;
    if show_size || show_location {
        let mut values = Vec::new();
        if show_size {
            values.push(info.installed_size.to_string());
        }
        if show_location {
            values.push(info.location.display().to_string());
        }
        println!("{}", values.join(" "));
        return Ok(());
    }

    println!("{:>15} {}", "ID:", info.id);
    println!("{:>15} {}", "Ref:", info.ref_name);
    println!("{:>15} {}", "Arch:", info.arch);
    println!("{:>15} {}", "Branch:", info.branch);
    println!("{:>15} {}", "Origin:", info.origin);
    println!("{:>15} user", "Installation:");
    println!(
        "{:>15} {}",
        "Installed Size:",
        super::size_format::format(info.installed_size)
    );
    if let Some(runtime) = info.runtime {
        println!("{:>15} {}", "Runtime:", runtime);
    }
    println!("{:>15} {}", "Commit:", info.commit);
    Ok(())
}

fn resolve_installed(
    paths: &Installation,
    name: &str,
    branch: Option<&str>,
) -> Result<InstalledInfo> {
    let partial = PartialRef::parse(name)?.with_default_branch(branch)?;
    let mut matches = Vec::new();
    for app in state::list_apps(paths)? {
        if partial.matches(&FlatpakRef::parse(&app.app_ref)?) {
            matches.push(InstalledInfo {
                id: app.app_id,
                ref_name: app.app_ref,
                arch: app.arch,
                branch: app.branch,
                origin: app.origin,
                commit: app.app_commit,
                installed_size: app.installed_size,
                location: paths.absolute_data_path(&app.app_dir),
                runtime: Some(app.runtime_ref),
            });
        }
    }
    for runtime in state::list_runtimes(paths)? {
        let (id, arch, ref_branch) = split_runtime_ref(&runtime.runtime_ref)?;
        let full_ref = format!("runtime/{}", runtime.runtime_ref);
        if partial.matches(&FlatpakRef::parse(&full_ref)?) {
            matches.push(InstalledInfo {
                id,
                ref_name: full_ref,
                arch,
                branch: ref_branch,
                origin: runtime.origin,
                commit: runtime.runtime_commit,
                installed_size: runtime.installed_size,
                location: paths.absolute_data_path(&runtime.runtime_dir),
                runtime: None,
            });
        }
    }
    for extension in state::list_extensions(paths)? {
        let runtime_ref = extension
            .ref_name
            .strip_prefix("runtime/")
            .unwrap_or(&extension.ref_name);
        let (id, arch, ref_branch) = split_runtime_ref(runtime_ref)?;
        if partial.matches(&FlatpakRef::parse(&extension.ref_name)?) {
            matches.push(InstalledInfo {
                id,
                ref_name: extension.ref_name,
                arch,
                branch: ref_branch,
                origin: extension.origin,
                commit: extension.commit,
                installed_size: extension.installed_size,
                location: extension.checkout_dir,
                runtime: None,
            });
        }
    }
    match matches.len() {
        0 => bail!("{name} is not installed"),
        1 => Ok(matches.remove(0)),
        _ => bail!("{name} matches multiple installed refs; specify a full ref or branch"),
    }
}

fn split_runtime_ref(value: &str) -> Result<(String, String, String)> {
    let mut parts = value.splitn(3, '/');
    let id = parts.next().unwrap_or_default();
    let arch = parts.next().unwrap_or_default();
    let branch = parts.next().unwrap_or_default();
    if id.is_empty() || arch.is_empty() || branch.is_empty() {
        bail!("invalid installed runtime ref: {value}");
    }
    Ok((id.to_string(), arch.to_string(), branch.to_string()))
}
