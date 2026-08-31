mod confirmation;
mod error_output;
mod help;
mod info;
mod install;
mod kill;
mod list;
mod list_table;
mod permissions;
mod prune;
mod ps;
#[path = "remotes.rs"]
mod remote_commands;
mod remote_info;
mod repair;
mod run;
mod search;
mod size_format;
mod style;
mod uninstall;
mod update;
mod update_output;
use crate::diagnostics::{Detail, Diagnostics, Verbosity};

pub(crate) use error_output::report as report_error;

use crate::installation::startup_recovery as startup;
use crate::remotes;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;

pub(crate) fn run_at_process_boundary() -> Result<()> {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !is_stdout_broken_pipe(info.payload()) {
            default_hook(info);
        }
    }));

    match std::panic::catch_unwind(run) {
        Ok(result) => result,
        Err(payload) if is_stdout_broken_pipe(payload.as_ref()) => Ok(()),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

fn is_stdout_broken_pipe(payload: &(dyn std::any::Any + Send)) -> bool {
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied());
    message.is_some_and(|message| {
        message.contains("failed printing to stdout")
            && (message.contains("Broken pipe") || message.contains("os error 32"))
    })
}

pub(crate) fn run() -> Result<()> {
    let raw_args = std::env::args().skip(1).collect::<Vec<_>>();
    let (verbosity, all_args) = parse_global_options(raw_args);
    let diagnostics = Diagnostics::new(verbosity);
    let mut args = all_args.clone().into_iter();
    let command = args.next();
    if matches!(command.as_deref(), Some("-h" | "--help")) {
        help::print_help();
        return Ok(());
    }
    let command_args = all_args.get(1..).unwrap_or_default();
    if command_args == ["-h"]
        || command_args == ["--help"]
        || (matches!(command.as_deref(), Some("list" | "info"))
            && command_args
                .iter()
                .any(|arg| matches!(arg.as_str(), "-h" | "--help")))
    {
        let handled = match command.as_deref() {
            Some("install") => help::print_install_help(),
            Some("info") => help::print_info_help(),
            Some("kill") => help::print_kill_help(),
            Some("list") => help::print_list_help(),
            Some("update" | "upgrade") => help::print_update_help(),
            Some("uninstall" | "remove") => help::print_uninstall_help(),
            Some("remotes") => help::print_remotes_help(),
            Some("remote-add") => help::print_remote_add_help(),
            Some("remote-modify") => help::print_remote_modify_help(),
            Some("remote-delete") => help::print_remote_delete_help(),
            Some("remote-ls") => help::print_remote_ls_help(),
            Some("remote-info") => help::print_remote_info_help(),
            _ => false,
        };
        if handled {
            return Ok(());
        }
    }

    let paths = match command.as_deref() {
        Some("kill" | "ps") => startup::initialize_for_lifecycle(&diagnostics),
        Some("run") => diagnostics.measure(Detail::Summary, "run", "installation startup", || {
            startup::initialize_for_run(&diagnostics)
        }),
        _ => startup::initialize(&diagnostics),
    }?;
    match command.as_deref() {
        Some("search") => search::cmd_search(&paths, args.collect()),
        Some("install") => install::cmd_install(&paths, args.collect(), &diagnostics),
        Some("info") => info::cmd_info(&paths, args.collect()),
        Some("kill") => kill::cmd_kill(&paths, args.collect()),
        Some("list") => list::cmd_list(&paths, args.collect()),
        Some("permissions") => permissions::cmd_permissions(&paths, args.collect()),
        Some("ps") => ps::cmd_ps(&paths, args.collect()),
        Some("prune") => prune::cmd_prune(&paths, args.collect()),
        Some("repair") => repair::cmd_repair(&paths, args.collect()),
        Some("run") => run::cmd_run(&paths, args.collect(), &diagnostics),
        Some("remote-info") => remote_info::cmd_remote_info(&paths, args.collect()),
        Some("remotes") => remote_commands::cmd_remotes(&paths, args.collect()),
        Some("remote-add") => remote_commands::cmd_remote_add(&paths, args.collect()),
        Some("remote-delete") => remote_commands::cmd_remote_delete(&paths, args.collect()),
        Some("remote-modify") => remote_commands::cmd_remote_modify(&paths, args.collect()),
        Some("remote-ls") => remote_commands::cmd_remote_ls(&paths, args.collect()),
        Some("uninstall" | "remove") => uninstall::cmd_uninstall(&paths, args.collect()),
        Some("update" | "upgrade") => update::cmd_update(&paths, args.collect(), &diagnostics),
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

fn parse_global_options(args: Vec<String>) -> (Verbosity, Vec<String>) {
    let mut verbosity = Verbosity::default();
    let mut remaining = args.into_iter();
    let mut parsed = Vec::new();

    for arg in remaining.by_ref() {
        let count = match arg.as_str() {
            "--verbose" => 1,
            _ if arg.starts_with('-')
                && arg.len() > 1
                && arg[1..].chars().all(|flag| flag == 'v') =>
            {
                arg.len() - 1
            }
            _ => {
                parsed.push(arg);
                break;
            }
        };
        for _ in 0..count {
            verbosity.increment();
        }
    }
    parsed.extend(remaining);
    (verbosity, parsed)
}

#[cfg(test)]
#[path = "tests/support.rs"]
mod test_support;

#[cfg(test)]
mod boundary_tests {
    use super::*;

    #[test]
    fn recognizes_only_stdout_broken_pipe_panics() {
        assert!(is_stdout_broken_pipe(
            &"failed printing to stdout: Broken pipe (os error 32)".to_string()
        ));
        assert!(!is_stdout_broken_pipe(&"unrelated panic".to_string()));
        assert!(!is_stdout_broken_pipe(
            &"failed printing to stderr: Broken pipe (os error 32)".to_string()
        ));
    }
}

#[cfg(test)]
#[path = "tests/error_output.rs"]
mod error_output_tests;

#[cfg(test)]
#[path = "tests/verbosity.rs"]
mod verbosity_tests;
