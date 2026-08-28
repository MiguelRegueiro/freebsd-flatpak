use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "freebsd-flatpak-info-errors-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn run_info(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_flatpak"))
            .arg("info")
            .args(args)
            .env("HOME", self.root.join("home"))
            .env("FREEBSD_FLATPAK_DATA_DIR", self.root.join("data"))
            .env("FREEBSD_FLATPAK_CACHE_DIR", self.root.join("cache"))
            .env("FREEBSD_FLATPAK_RUNTIME_DIR", self.root.join("runtime"))
            .env("FREEBSD_FLATPAK_APP_DATA_DIR", self.root.join("app-data"))
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn assert_info_error(output: Output, expected: &str) {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(String::from_utf8(output.stderr).unwrap(), expected);
}

#[test]
fn valid_uninstalled_id_uses_flatpak_error_format_and_fails() {
    let fixture = Fixture::new();
    assert_info_error(
        fixture.run_info(&["io.github.Faugus.faugus-launcher"]),
        "error: io.github.Faugus.faugus-launcher is not installed\n",
    );
}

#[test]
fn invalid_id_is_rejected_before_installed_ref_lookup() {
    let fixture = Fixture::new();
    assert_info_error(
        fixture.run_info(&["ddd"]),
        "error: Invalid id ddd: Names must contain at least 2 periods\n",
    );
}

#[test]
fn full_refs_apply_flatpak_id_validation() {
    let fixture = Fixture::new();
    assert_info_error(
        fixture.run_info(&["app/org.invalid-id.App/x86_64/stable"]),
        "error: Invalid id org.invalid-id.App: Only last name segment can contain -\n",
    );
}

#[test]
fn branch_operands_apply_flatpak_branch_validation() {
    let fixture = Fixture::new();
    assert_info_error(
        fixture.run_info(&["org.example.App", ".invalid"]),
        "error: Invalid branch .invalid: Branch can't start with \".\"\n",
    );
}
