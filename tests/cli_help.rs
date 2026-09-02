use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};

const EXPECTED_HELP: &str = r#"Usage:
  flatpak [OPTION] COMMAND

Commands:
  install       Install an application or runtime
  info          Show information about an installed ref
  update        Update installed applications and runtimes
  remotes       List configured remotes
  remote-add    Add a remote
  remote-delete Delete a remote
  remote-modify Modify a remote
  remote-ls     List refs in remotes
  remote-info   Show information about an application in a remote
  uninstall     Uninstall an application or runtime
  list          List installed applications and runtimes
  search        Search configured remotes
  run           Run an application
  ps            List running applications
  kill          Stop a running application
  permissions   Show application permissions
  repair        Verify and repair the installation
  prune         Remove unused stored data

Options:
  -v, --verbose Show diagnostics; use -vv for detailed diagnostics
  -h, --help    Show help
"#;

const EXPECTED_UNINSTALL_HELP: &str = r#"Usage:
  flatpak uninstall [OPTION] [REF]

Options:
  --app                 Look for an application ref
  --runtime             Look for a runtime ref
  --unused             Remove unused runtime and extension refs
  --delete-data        Delete app data
  --no-related         Don't uninstall related extensions
  -y, --assumeyes      Automatically answer yes for all questions
  --noninteractive     Produce minimal output and don't ask questions
  -h, --help           Show help
"#;

const EXPECTED_INSTALL_HELP: &str = r#"Usage:
  flatpak install [OPTION] [REMOTE] REF

Options:
  --app                Look for an application ref
  --runtime            Look for a runtime ref
  --or-update          Update install if already installed
  --no-related         Don't install related extensions
  -y, --assumeyes      Automatically answer yes for all questions
  --noninteractive     Produce minimal output and don't ask questions
  -h, --help           Show help
"#;

const EXPECTED_UPDATE_HELP: &str = r#"Usage:
  flatpak update [OPTION] [REF...]

Options:
  --app                 Update application refs
  --runtime             Update runtime refs
  --commit=COMMIT      Update to this commit
  --no-related         Don't update related extensions
  -y, --assumeyes      Automatically answer yes for all questions
  --noninteractive     Produce minimal output and don't ask questions
  -h, --help           Show help
"#;
const EXPECTED_KILL_HELP: &str = r#"Usage:
  flatpak kill INSTANCE

Stop a running application by application ID or instance ID.

Options:
  -h, --help    Show help
"#;

#[test]
fn top_level_help_flags_print_help_without_initializing_an_installation() {
    for flag in ["-h", "--help"] {
        let output = Command::new(env!("CARGO_BIN_EXE_flatpak"))
            .arg(flag)
            .env_remove("HOME")
            .output()
            .unwrap();

        assert!(output.status.success(), "{flag} failed: {output:?}");
        assert_eq!(String::from_utf8(output.stdout).unwrap(), EXPECTED_HELP);
        assert!(output.stderr.is_empty(), "{flag} wrote to stderr");
    }
}

#[test]
fn uninstall_help_documents_unused_without_initializing_an_installation() {
    for flag in ["-h", "--help"] {
        let output = Command::new(env!("CARGO_BIN_EXE_flatpak"))
            .args(["uninstall", flag])
            .env_remove("HOME")
            .output()
            .unwrap();

        assert!(output.status.success(), "{flag} failed: {output:?}");
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            EXPECTED_UNINSTALL_HELP
        );
        assert!(output.stderr.is_empty(), "{flag} wrote to stderr");
    }
}

#[test]
fn transaction_command_help_documents_visible_options() {
    for (command, expected) in [
        ("install", EXPECTED_INSTALL_HELP),
        ("update", EXPECTED_UPDATE_HELP),
        ("uninstall", EXPECTED_UNINSTALL_HELP),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_flatpak"))
            .args([command, "--help"])
            .env_remove("HOME")
            .output()
            .unwrap();
        assert!(output.status.success(), "{command} failed: {output:?}");
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn kill_help_is_available_without_initializing_an_installation() {
    for flag in ["-h", "--help"] {
        let output = Command::new(env!("CARGO_BIN_EXE_flatpak"))
            .args(["kill", flag])
            .env_remove("HOME")
            .output()
            .unwrap();

        assert!(output.status.success(), "{flag} failed: {output:?}");
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            EXPECTED_KILL_HELP
        );
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn run_help_documents_runtime_overrides_without_initializing_an_installation() {
    let output = Command::new(env!("CARGO_BIN_EXE_flatpak"))
        .args(["run", "--help"])
        .env_remove("HOME")
        .output()
        .unwrap();
    assert!(output.status.success(), "run --help failed: {output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("--runtime=RUNTIME"));
    assert!(stdout.contains("--runtime-version=BRANCH"));
    assert!(output.stderr.is_empty());
}
#[test]
fn compatibility_aliases_work_but_stay_hidden_from_top_level_help() {
    for (alias, expected) in [
        ("upgrade", EXPECTED_UPDATE_HELP),
        ("remove", EXPECTED_UNINSTALL_HELP),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_flatpak"))
            .args([alias, "--help"])
            .env_remove("HOME")
            .output()
            .unwrap();
        assert!(output.status.success(), "{alias} failed: {output:?}");
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
    }
    assert!(!EXPECTED_HELP.contains("upgrade"));
    assert!(!EXPECTED_HELP.contains("remove"));
}

#[test]
fn remote_command_help_is_recognized_before_initialization_and_operand_parsing() {
    for (command, usage) in [
        ("remotes", "Usage:\n  flatpak remotes\n"),
        (
            "remote-add",
            "Usage:\n  flatpak remote-add [OPTION] NAME LOCATION\n",
        ),
        (
            "remote-modify",
            "Usage:\n  flatpak remote-modify [OPTION] NAME\n",
        ),
        (
            "remote-delete",
            "Usage:\n  flatpak remote-delete [OPTION] NAME\n",
        ),
        ("remote-ls", "Usage:\n  flatpak remote-ls [REMOTE]\n"),
        (
            "remote-info",
            "Usage:\n  flatpak remote-info [OPTION] REMOTE REF\n",
        ),
    ] {
        for flag in ["-h", "--help"] {
            let output = Command::new(env!("CARGO_BIN_EXE_flatpak"))
                .args([command, flag])
                .env_remove("HOME")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{command} {flag} failed: {output:?}"
            );
            let stdout = String::from_utf8(output.stdout).unwrap();
            assert!(
                stdout.starts_with(usage),
                "unexpected {command} help: {stdout}"
            );
            assert!(stdout.contains("-h, --help"));
            assert!(output.stderr.is_empty(), "{command} {flag} wrote to stderr");
        }
    }
}

#[test]
fn installed_ref_command_help_is_recognized_before_initialization() {
    for (command, options) in [
        ("list", ["--all", "--app-runtime"]),
        ("info", ["--show-size", "--show-extensions"]),
    ] {
        for flag in ["-h", "--help"] {
            let output = Command::new(env!("CARGO_BIN_EXE_flatpak"))
                .args([command, flag])
                .env_remove("HOME")
                .output()
                .unwrap();
            assert!(output.status.success(), "{command} {flag}: {output:?}");
            let stdout = String::from_utf8(output.stdout).unwrap();
            assert!(stdout.starts_with(&format!("Usage:\n  flatpak {command}")));
            assert!(stdout.contains(options[0]));
            assert!(stdout.contains(options[1]));
        }
    }
}

#[test]
fn closed_stdout_is_a_successful_quiet_exit() {
    let (reader, writer) = UnixStream::pair().unwrap();
    drop(reader);
    let writer: OwnedFd = writer.into();
    let output = Command::new(env!("CARGO_BIN_EXE_flatpak"))
        .args(["remote-add", "--help"])
        .env_remove("HOME")
        .stdout(Stdio::from(writer))
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(output.status.success(), "broken stdout failed: {output:?}");
    assert!(output.stderr.is_empty(), "broken stdout was noisy");
}

#[test]
fn verbosity_flags_are_global_and_do_not_change_help_behavior() {
    for args in [
        &["-v", "--help"][..],
        &["-vv", "--help"][..],
        &["-v", "--verbose", "--help"][..],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_flatpak"))
            .args(args)
            .env_remove("HOME")
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?} failed: {output:?}");
        assert_eq!(String::from_utf8(output.stdout).unwrap(), EXPECTED_HELP);
        assert!(output.stderr.is_empty());
    }
}
