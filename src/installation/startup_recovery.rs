use crate::host_resources::graphics;
use crate::installation as state;
use crate::installation::{self as runtime, installation_paths::Installation};
use crate::{portal_integration, remotes, sandbox};
use anyhow::Result;

pub(crate) fn initialize() -> Result<Installation> {
    let paths = Installation::from_env()?;
    state::ensure_layout(&paths)?;
    remotes::initialize(&paths)?;
    sandbox::recover_stale_mounts(&paths)?;
    portal_integration::recover_stale_portal_mounts(&paths)?;
    graphics::recover_stale_graphics_dirs(&paths)?;
    runtime::recover_storage(&paths)?;
    state::reconcile_runtime_bindings(&paths)?;
    state::cleanup_retired_deployments(&paths)?;
    Ok(paths)
}
