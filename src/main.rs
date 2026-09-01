mod architecture;
mod cli;
mod desktop_integration;
mod diagnostics;
mod extensions;
mod flatpak_metadata;
mod flatpak_ref;
mod host_resources;
mod installation;
mod ostree;
mod portal_integration;
mod process_identity;
mod remotes;
mod sandbox;
#[allow(dead_code)]
mod secure_launch;
#[allow(dead_code)]
mod secure_mount;

fn main() -> std::process::ExitCode {
    match cli::run_at_process_boundary() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            cli::report_error(&error);
            std::process::ExitCode::FAILURE
        }
    }
}
