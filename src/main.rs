mod cli;
mod desktop_integration;
mod diagnostics;
mod extensions;
mod flatpak_metadata;
mod host_resources;
mod installation;
mod ostree;
mod portal_integration;
mod remotes;
mod sandbox;

fn main() -> anyhow::Result<()> {
    cli::run_at_process_boundary()
}
