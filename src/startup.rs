use crate::{graphics, paths::Installation, portal, runtime, sandbox, state};
use anyhow::Result;

pub(crate) fn initialize() -> Result<Installation> {
    let paths = Installation::from_env()?;
    state::ensure_layout(&paths)?;
    sandbox::recover_stale_mounts(&paths)?;
    portal::recover_stale_portal_mounts(&paths)?;
    graphics::recover_stale_graphics_dirs(&paths)?;
    runtime::recover_storage(&paths)?;
    state::reconcile_runtime_bindings(&paths)?;
    state::cleanup_retired_deployments(&paths)?;
    Ok(paths)
}
