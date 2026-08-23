mod help;
pub(crate) mod transaction;

use crate::{commands, startup};
use anyhow::{bail, Result};

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
        Some("search") => commands::cmd_search(&paths, args.collect()),
        Some("install") => commands::cmd_install(&paths, args.collect()),
        Some("list") => commands::cmd_list(&paths, args.collect()),
        Some("permissions") => commands::cmd_permissions(&paths, args.collect()),
        Some("ps") => commands::cmd_ps(&paths, args.collect()),
        Some("prune") => commands::cmd_prune(&paths, args.collect()),
        Some("repair") => commands::cmd_repair(&paths, args.collect()),
        Some("run") => commands::cmd_run(&paths, args.collect()),
        Some("remote-info") => commands::cmd_remote_info(&paths, args.collect()),
        Some("uninstall" | "remove") => commands::cmd_uninstall(&paths, args.collect()),
        Some("update" | "upgrade") => commands::cmd_update(&paths, args.collect()),
        Some("checkout") => commands::internal::checkout(&paths, args),
        Some("inspect") => commands::internal::inspect(&paths, args.collect()),
        Some(cmd) => bail!("unknown command: {cmd}"),
        None => {
            help::print_usage();
            Ok(())
        }
    }
}
