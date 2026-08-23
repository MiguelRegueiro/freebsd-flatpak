mod install;
pub(crate) mod internal;
mod list;
mod maintenance;
mod permissions;
mod remote_info;
mod run;
mod search;
mod uninstall;
mod update;

pub(crate) use install::cmd_install;
pub(crate) use list::cmd_list;
pub(crate) use maintenance::{cmd_prune, cmd_repair};
pub(crate) use permissions::cmd_permissions;
pub(crate) use remote_info::cmd_remote_info;
pub(crate) use run::cmd_run;
pub(crate) use search::cmd_search;
pub(crate) use uninstall::cmd_uninstall;
pub(crate) use update::cmd_update;

use crate::{paths::Installation, ps};
use anyhow::Result;

pub(crate) fn cmd_ps(paths: &Installation, args: Vec<String>) -> Result<()> {
    print!("{}", ps::output(paths, args)?);
    Ok(())
}

fn value_after_equals(arg: &str) -> &str {
    arg.split_once('=').map(|(_, value)| value).unwrap_or("")
}

#[cfg(test)]
#[path = "tests/support.rs"]
mod test_support;
