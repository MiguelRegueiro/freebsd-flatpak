const HELP: &str = r#"Usage:
  flatpak [OPTION] COMMAND

Commands:
  install       Install an application
  info          Show information about an installed ref
  update        Update installed applications
  remotes       List configured remotes
  remote-add    Add a remote
  remote-delete Delete a remote
  remote-modify Modify a remote
  remote-ls     List refs in remotes
  remote-info   Show information about an application in a remote
  uninstall     Uninstall an application
  list          List installed applications and runtimes
  search        Search configured remotes
  run           Run an application
  ps            List running applications
  permissions   Show application permissions
  repair        Verify and repair the installation
  prune         Remove unused stored data

Options:
  -v, --verbose  Show startup diagnostics; use -vv for detailed diagnostics
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
  flatpak install [OPTION] [REMOTE] APP-ID

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

const LIST_HELP: &str = r#"Usage:
  flatpak list [OPTION]

Options:
  --app                List installed applications
  --runtime            List installed runtimes and extensions
  -d, --show-details   Show all supported details (same as --columns=all)
  --columns=FIELD,...  Specify the columns to show; may be repeated
  -h, --help           Show help

Columns:
  application, arch, branch, runtime, ref, origin, installation,
  active, size, all, help
"#;

const INFO_HELP: &str = r#"Usage:
  flatpak info [OPTION] NAME [BRANCH]

Show information about an installed application or runtime.

Options:
  -s, --show-size      Show installed size in bytes
  -l, --show-location  Show deployment location
  -h, --help           Show help
"#;

const REMOTES_HELP: &str = r#"Usage:
  flatpak remotes

List configured remotes, including disabled remotes.

Options:
  -h, --help    Show help
"#;

const REMOTE_ADD_HELP: &str = r#"Usage:
  flatpak remote-add [OPTION] NAME LOCATION

Add a named repository URL or .flatpakrepo configuration.

Options:
  --if-not-exists      Do nothing if the remote already exists
  --disable            Add the remote disabled
  --title=TITLE        Set the remote title
  --gpg-import=FILE    Import a GPG key for this remote
  --gpg-verify         Require GPG verification
  --no-gpg-verify      Disable GPG verification
  -h, --help           Show help
"#;

const REMOTE_MODIFY_HELP: &str = r#"Usage:
  flatpak remote-modify [OPTION] NAME

Modify a configured remote.

Options:
  --enable             Enable the remote
  --disable            Disable the remote
  --url=URL            Change the repository URL
  --title=TITLE        Change the remote title
  --gpg-import=FILE    Import a GPG key for this remote
  --gpg-verify         Require GPG verification
  --no-gpg-verify      Disable GPG verification
  -h, --help           Show help
"#;

const REMOTE_DELETE_HELP: &str = r#"Usage:
  flatpak remote-delete [OPTION] NAME

Delete a configured remote.

Options:
  --force          Delete even when installed refs use the remote
  -h, --help       Show help
"#;

const REMOTE_LS_HELP: &str = r#"Usage:
  flatpak remote-ls [REMOTE]

List refs available from one remote or all enabled remotes.

Options:
  -h, --help    Show help
"#;

const REMOTE_INFO_HELP: &str = r#"Usage:
  flatpak remote-info [OPTION] REMOTE REF

Show information about an application ref in a remote.

Options:
  --log               Show commit history
  --commit=COMMIT     Show a historical commit
  -h, --help          Show help
"#;

pub(super) fn print_usage() {
    eprintln!("usage:");
    eprintln!("  flatpak search <query>");
    eprintln!("  flatpak install [OPTION] [REMOTE] <app-id>");
    eprintln!("  flatpak info [OPTION] NAME [BRANCH]");
    eprintln!("  flatpak remotes");
    eprintln!("  flatpak remote-add [OPTION] NAME LOCATION");
    eprintln!("  flatpak remote-delete [--force] NAME");
    eprintln!("  flatpak remote-modify [OPTION] NAME");
    eprintln!("  flatpak remote-ls [REMOTE]");
    eprintln!("  flatpak remote-info [--log | --commit=COMMIT] REMOTE <ref>");
    eprintln!("  flatpak list [--app | --runtime] [--columns=FIELD,...]");
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

pub(super) fn print_uninstall_help() -> bool {
    print!("{UNINSTALL_HELP}");
    true
}

pub(super) fn print_install_help() -> bool {
    print!("{INSTALL_HELP}");
    true
}

pub(super) fn print_list_help() -> bool {
    print!("{LIST_HELP}");
    true
}

pub(super) fn print_info_help() -> bool {
    print!("{INFO_HELP}");
    true
}

pub(super) fn print_update_help() -> bool {
    print!("{UPDATE_HELP}");
    true
}

pub(super) fn print_remotes_help() -> bool {
    print!("{REMOTES_HELP}");
    true
}

pub(super) fn print_remote_add_help() -> bool {
    print!("{REMOTE_ADD_HELP}");
    true
}

pub(super) fn print_remote_modify_help() -> bool {
    print!("{REMOTE_MODIFY_HELP}");
    true
}

pub(super) fn print_remote_delete_help() -> bool {
    print!("{REMOTE_DELETE_HELP}");
    true
}

pub(super) fn print_remote_ls_help() -> bool {
    print!("{REMOTE_LS_HELP}");
    true
}

pub(super) fn print_remote_info_help() -> bool {
    print!("{REMOTE_INFO_HELP}");
    true
}
