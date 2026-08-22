use std::process::Command;

const EXPECTED_HELP: &str = r#"Usage:
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

const EXPECTED_UNINSTALL_HELP: &str = r#"Usage:
  flatpak uninstall [OPTION] [APP-ID]

Options:
  --unused      Remove unused runtime and extension refs
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
