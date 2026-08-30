use crate::diagnostics::{Detail, Diagnostics};
use crate::host_resources::graphics;
use crate::installation as state;
use crate::installation::{self as runtime, installation_paths::Installation};
use crate::{portal_integration, remotes, sandbox};
use anyhow::Result;

pub(crate) fn initialize(diagnostics: &Diagnostics) -> Result<Installation> {
    initialize_inner(diagnostics, true)
}

// Lifecycle commands only inspect already-published run records. Avoid remote
// initialization and broad stale-resource recovery so process control remains
// responsive while another launcher is still constructing its sandbox.
pub(crate) fn initialize_for_lifecycle(diagnostics: &Diagnostics) -> Result<Installation> {
    let paths = diagnostics.measure(
        Detail::Detailed,
        "installation",
        "resolve paths",
        Installation::from_env,
    )?;
    diagnostics.measure(
        Detail::Detailed,
        "installation",
        "ensure state layout",
        || state::ensure_layout(&paths),
    )?;
    Ok(paths)
}

// `run` resolves already-installed deployments from local state and never uses
// OSTree remotes. Keep crash and transient-resource recovery below, but avoid
// rewriting remote configuration and importing trust keys on every launch.
pub(crate) fn initialize_for_run(diagnostics: &Diagnostics) -> Result<Installation> {
    initialize_inner(diagnostics, false)
}

fn initialize_inner(diagnostics: &Diagnostics, initialize_remotes: bool) -> Result<Installation> {
    let paths = diagnostics.measure(
        Detail::Detailed,
        "installation",
        "resolve paths",
        Installation::from_env,
    )?;
    diagnostics.measure(
        Detail::Detailed,
        "installation",
        "ensure state layout",
        || state::ensure_layout(&paths),
    )?;
    if initialize_remotes {
        diagnostics.measure(
            Detail::Detailed,
            "installation",
            "initialize remotes",
            || remotes::initialize_detailed(&paths, diagnostics),
        )?;
    }
    diagnostics.measure(
        Detail::Detailed,
        "installation",
        "recover sandbox mounts",
        || sandbox::recover_stale_mounts(&paths),
    )?;
    diagnostics.measure(
        Detail::Detailed,
        "installation",
        "recover portal state",
        || portal_integration::recover_stale_portal_mounts(&paths),
    )?;
    diagnostics.measure(
        Detail::Detailed,
        "installation",
        "recover graphics state",
        || graphics::recover_stale_graphics_dirs(&paths),
    )?;
    diagnostics.measure(
        Detail::Detailed,
        "installation",
        "recover OSTree storage",
        || runtime::recover_storage(&paths),
    )?;
    diagnostics.measure(
        Detail::Detailed,
        "installation",
        "reconcile runtime bindings",
        || state::reconcile_runtime_bindings(&paths),
    )?;
    diagnostics.measure(
        Detail::Detailed,
        "installation",
        "cleanup retired deployments",
        || state::cleanup_retired_deployments(&paths),
    )?;
    Ok(paths)
}
