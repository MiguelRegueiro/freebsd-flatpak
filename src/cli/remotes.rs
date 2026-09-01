use crate::installation as state;
use crate::installation::installation_paths::Installation;
use crate::remotes;
use anyhow::{bail, Context, Result};

pub(crate) fn cmd_remotes(paths: &Installation, args: Vec<String>) -> Result<()> {
    if !args.is_empty() {
        bail!("usage: flatpak remotes");
    }
    println!("{:<20} {:<24} {:<8} URL", "Name", "Title", "Enabled");
    for remote in remotes::list_remotes(paths)? {
        println!(
            "{:<20} {:<24} {:<8} {}",
            remote.name,
            remote.title.as_deref().unwrap_or(""),
            if remote.enabled { "yes" } else { "no" },
            remote.url
        );
    }
    Ok(())
}

pub(crate) fn cmd_remote_add(paths: &Installation, args: Vec<String>) -> Result<()> {
    let mut if_not_exists = false;
    let mut enabled = true;
    let mut gpg_verify = None;
    let mut title = None;
    let mut gpg_import = None;
    let mut operands = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--if-not-exists" => if_not_exists = true,
            "--disable" => enabled = false,
            "--no-gpg-verify" => gpg_verify = Some(false),
            "--gpg-verify" => gpg_verify = Some(true),
            "--title" => title = Some(args.next().context("missing value for --title")?),
            "--gpg-import" => {
                gpg_import = Some(args.next().context("missing value for --gpg-import")?)
            }
            _ if arg.starts_with("--title=") => title = Some(value(&arg).to_string()),
            _ if arg.starts_with("--gpg-import=") => gpg_import = Some(value(&arg).to_string()),
            _ if arg.starts_with('-') => bail!("unknown remote-add option: {arg}"),
            _ => operands.push(arg),
        }
    }
    if operands.len() != 2 {
        bail!("usage: flatpak remote-add [OPTION] NAME LOCATION");
    }
    let name = operands.remove(0);
    let location = operands.remove(0);
    let mut remote = remotes::from_location(name, &location)?;
    remote.enabled = enabled;
    if let Some(title) = title {
        remote.title = Some(title);
    }
    if let Some(location) = gpg_import {
        remote.gpg_key = Some(remotes::read_gpg_key(&location)?);
        remote.gpg_verify = true;
    }
    if let Some(verify) = gpg_verify {
        remote.gpg_verify = verify;
    }
    if remotes::add_remote(paths, &remote, if_not_exists)? {
        println!("Added remote {}", remote.name);
    }
    Ok(())
}

pub(crate) fn cmd_remote_modify(paths: &Installation, args: Vec<String>) -> Result<()> {
    let mut operands = Vec::new();
    let mut enabled = None;
    let mut url = None;
    let mut title = None;
    let mut gpg_verify = None;
    let mut gpg_import = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--enable" => enabled = Some(true),
            "--disable" => enabled = Some(false),
            "--gpg-verify" => gpg_verify = Some(true),
            "--no-gpg-verify" => gpg_verify = Some(false),
            "--url" => url = Some(args.next().context("missing value for --url")?),
            "--title" => title = Some(args.next().context("missing value for --title")?),
            "--gpg-import" => {
                gpg_import = Some(args.next().context("missing value for --gpg-import")?)
            }
            _ if arg.starts_with("--url=") => url = Some(value(&arg).to_string()),
            _ if arg.starts_with("--title=") => title = Some(value(&arg).to_string()),
            _ if arg.starts_with("--gpg-import=") => gpg_import = Some(value(&arg).to_string()),
            _ if arg.starts_with('-') => bail!("unknown remote-modify option: {arg}"),
            _ => operands.push(arg),
        }
    }
    if operands.len() != 1 {
        bail!("usage: flatpak remote-modify [OPTION] NAME");
    }
    let mut remote = remotes::get_remote(paths, &operands[0])?;
    if let Some(enabled) = enabled {
        remote.enabled = enabled;
    }
    if let Some(url) = url {
        remote.url = url.trim_end_matches('/').to_string();
    }
    if let Some(title) = title {
        remote.title = Some(title);
    }
    if let Some(verify) = gpg_verify {
        remote.gpg_verify = verify;
    }
    if let Some(location) = gpg_import {
        remote.gpg_key = Some(remotes::read_gpg_key(&location)?);
        remote.gpg_verify = true;
    }
    remotes::modify_remote(paths, &remote)?;
    println!("Modified remote {}", remote.name);
    Ok(())
}

pub(crate) fn cmd_remote_delete(paths: &Installation, args: Vec<String>) -> Result<()> {
    let mut force = false;
    let mut operands = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--force" => force = true,
            _ if arg.starts_with('-') => bail!("unknown remote-delete option: {arg}"),
            _ => operands.push(arg),
        }
    }
    if operands.len() != 1 {
        bail!("usage: flatpak remote-delete [--force] NAME");
    }
    let name = &operands[0];
    remotes::get_remote(paths, name)?;
    let apps = state::list_apps(paths)?
        .into_iter()
        .filter(|record| record.origin == *name)
        .map(|record| record.app_id)
        .collect::<Vec<_>>();
    let runtimes = state::list_runtimes(paths)?
        .into_iter()
        .filter(|record| record.origin == *name)
        .map(|record| record.runtime_ref)
        .collect::<Vec<_>>();
    if !force && (!apps.is_empty() || !runtimes.is_empty()) {
        bail!("remote {name} is still referenced by installed refs (applications: {}; runtimes: {}); use --force to remove it", display(&apps), display(&runtimes));
    }
    remotes::delete_remote(paths, name)?;
    println!("Deleted remote {name}");
    Ok(())
}

pub(crate) fn cmd_remote_ls(paths: &Installation, args: Vec<String>) -> Result<()> {
    if args.len() > 1 {
        bail!("usage: flatpak remote-ls [REMOTE]");
    }
    let configured = if let Some(name) = args.first() {
        vec![remotes::get_remote(paths, name)?]
    } else {
        remotes::enabled_remotes(paths)?
    };
    println!("{:<20} {:<54} {:<8} Branch", "Remote", "Ref", "Arch");
    for remote in configured {
        if !remote.enabled {
            continue;
        }
        for item in remotes::load_remote_metadata(paths, &remote.name)?.list_refs() {
            println!(
                "{:<20} {:<54} {:<8} {}",
                item.remote, item.ref_name, item.arch, item.branch
            );
        }
    }
    Ok(())
}

fn value(arg: &str) -> &str {
    arg.split_once('=').map(|(_, value)| value).unwrap_or("")
}
fn display(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installation::AppRecord;
    use std::path::PathBuf;

    #[test]
    fn referenced_remote_requires_force() {
        let root = std::env::temp_dir().join(format!(
            "remote-delete-safety-{}-{}",
            std::process::id(),
            crate::remotes::unique_sequence()
        ));
        let paths = Installation::for_test(&root);
        state::ensure_layout(&paths).unwrap();
        for name in ["example", "runtime-source"] {
            remotes::add_remote(
                &paths,
                &remotes::Remote {
                    name: name.to_string(),
                    url: format!("https://{name}.example/repo"),
                    title: None,
                    enabled: true,
                    gpg_verify: false,
                    gpg_key: None,
                },
                false,
            )
            .unwrap();
        }
        state::write_app(
            &paths,
            &AppRecord {
                origin: "example".to_string(),
                runtime_origin: "runtime-source".to_string(),
                app_id: "org.example.App".to_string(),
                app_ref: "app/org.example.App/x86_64/stable".to_string(),
                app_commit: "commit".to_string(),
                installed_size: 0,
                app_dir: PathBuf::from("apps/org.example.App/commit"),
                arch: "x86_64".to_string(),
                branch: "stable".to_string(),
                runtime_ref: "org.example.Platform/x86_64/stable".to_string(),
                runtime_commit: "runtime".to_string(),
                runtime_dir: PathBuf::from("runtimes/example/runtime"),
                command: "example".to_string(),
            },
        )
        .unwrap();
        let error = cmd_remote_delete(&paths, vec!["example".to_string()]).unwrap_err();
        state::write_runtime(
            &paths,
            &state::RuntimeRecord {
                origin: "runtime-source".to_string(),
                runtime_ref: "org.example.Platform/x86_64/stable".to_string(),
                runtime_commit: "runtime".to_string(),
                installed_size: 0,
                explicitly_installed: false,
                runtime_dir: PathBuf::from("runtimes/example/runtime"),
            },
        )
        .unwrap();
        assert!(error.to_string().contains("still referenced"));
        let error = cmd_remote_delete(&paths, vec!["runtime-source".to_string()]).unwrap_err();
        assert!(error.to_string().contains("still referenced"));
        let _ = std::fs::remove_dir_all(root);
    }
}
