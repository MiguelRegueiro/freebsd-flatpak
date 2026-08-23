mod confirmation;
mod help;
mod install;
mod list;
mod permissions;
mod prune;
mod ps;
mod remote_info;
mod repair;
mod run;
mod search;
mod uninstall;
mod update;

use crate::installation::startup_recovery as startup;
use crate::remotes;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;

pub(crate) fn run() -> Result<()> {
    let all_args = std::env::args().skip(1).collect::<Vec<_>>();
    let mut args = all_args.clone().into_iter();
    let command = args.next();
    if matches!(command.as_deref(), Some("-h" | "--help")) {
        help::print_help();
        return Ok(());
    }
    let command_args = all_args.get(1..).unwrap_or_default();
    if command_args == ["-h"] || command_args == ["--help"] {
        match command.as_deref() {
            Some("install") => help::print_install_help(),
            Some("update" | "upgrade") => help::print_update_help(),
            Some("uninstall" | "remove") => help::print_uninstall_help(),
            _ => {}
        }
        if matches!(
            command.as_deref(),
            Some("install" | "update" | "upgrade" | "uninstall" | "remove")
        ) {
            return Ok(());
        }
    }

    let paths = startup::initialize()?;
    match command.as_deref() {
        Some("search") => search::cmd_search(&paths, args.collect()),
        Some("install") => install::cmd_install(&paths, args.collect()),
        Some("list") => list::cmd_list(&paths, args.collect()),
        Some("permissions") => permissions::cmd_permissions(&paths, args.collect()),
        Some("ps") => ps::cmd_ps(&paths, args.collect()),
        Some("prune") => prune::cmd_prune(&paths, args.collect()),
        Some("repair") => repair::cmd_repair(&paths, args.collect()),
        Some("run") => run::cmd_run(&paths, args.collect()),
        Some("remote-info") => remote_info::cmd_remote_info(&paths, args.collect()),
        Some("uninstall" | "remove") => uninstall::cmd_uninstall(&paths, args.collect()),
        Some("update" | "upgrade") => update::cmd_update(&paths, args.collect()),
        Some("checkout") => {
            let ref_name = args.next().context("missing ref")?;
            let dest = args.next().context("missing destination")?;
            remotes::checkout_ref(&paths, &ref_name, PathBuf::from(dest))
        }
        Some("inspect") => {
            let refs = args.collect::<Vec<_>>();
            remotes::inspect_refs(&paths, &refs)
        }
        Some(cmd) => bail!("unknown command: {cmd}"),
        None => {
            help::print_usage();
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "tests/support.rs"]
mod test_support;
