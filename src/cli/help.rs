const HELP: &str = r#"Usage:
  flatpak [OPTION] COMMAND

Commands:
  install       Install an application
  update        Update installed applications
  remote-info   Show information about an application in a remote
  uninstall     Uninstall an application
  list          List installed applications
  search        Search Flathub
  run           Run an application
  ps            List running applications
  permissions   Show application permissions
  repair        Verify and repair the installation
  prune         Remove unused stored data

Options:
  -h, --help    Show help
"#;

const UNINSTALL_HELP: &str = r#"Usage:
  flatpak uninstall [OPTION] [APP-ID]

Options:
  --unused             Remove unused runtime and extension refs
  --delete-data        Delete app data
  -y, --assumeyes      Automatically answer yes for all questions
  --noninteractive     Produce minimal output and don't ask questions
  -h, --help           Show help
"#;

const INSTALL_HELP: &str = r#"Usage:
  flatpak install [OPTION] APP-ID

Options:
  --or-update          Update install if already installed
  -y, --assumeyes      Automatically answer yes for all questions
  --noninteractive     Produce minimal output and don't ask questions
  -h, --help           Show help
"#;

const UPDATE_HELP: &str = r#"Usage:
  flatpak update [OPTION] [APP-ID...]

Options:
  --commit=COMMIT      Update to this commit
  -y, --assumeyes      Automatically answer yes for all questions
  --noninteractive     Produce minimal output and don't ask questions
  -h, --help           Show help
"#;

pub(super) fn print_usage() {
    eprintln!("usage:");
    eprintln!("  flatpak search <query>");
    eprintln!("  flatpak install [OPTION] <app-id>");
    eprintln!("  flatpak remote-info [--log | --commit=COMMIT] flathub <app-id>");
    eprintln!("  flatpak list");
    eprintln!("  flatpak permissions <app-id>");
    eprintln!("  flatpak ps [--columns=FIELD,...]");
    eprintln!("  flatpak prune");
    eprintln!("  flatpak repair");
    eprintln!("  flatpak run <app-id> [-- app-args...]");
    eprintln!("  flatpak uninstall [OPTION] [--unused | <app-id>]");
    eprintln!("  flatpak update [OPTION] [app-id...]");
}

pub(super) fn print_help() {
    print!("{HELP}");
}

pub(super) fn print_uninstall_help() {
    print!("{UNINSTALL_HELP}");
}

pub(super) fn print_install_help() {
    print!("{INSTALL_HELP}");
}

pub(super) fn print_update_help() {
    print!("{UPDATE_HELP}");
}
