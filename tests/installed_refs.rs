use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    data: PathBuf,
    app_location: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "freebsd-flatpak-installed-refs-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let data = root.join("data");
        let refs = data.join("refs");
        fs::create_dir_all(refs.join("apps")).unwrap();
        fs::create_dir_all(refs.join("runtimes")).unwrap();
        let app_location = data.join("apps/app.zen_browser.zen/app-commit");
        let runtime_location = data.join("runtimes/flathub/platform/runtime-commit");
        fs::create_dir_all(&app_location).unwrap();
        fs::create_dir_all(&runtime_location).unwrap();
        let app_metainfo = app_location.join("files/share/metainfo");
        let runtime_metainfo = runtime_location.join("files/share/metainfo");
        fs::create_dir_all(&app_metainfo).unwrap();
        fs::create_dir_all(&runtime_metainfo).unwrap();
        fs::write(
            app_metainfo.join("app.zen_browser.zen.metainfo.xml"),
            r#"<component><id>app.zen_browser.zen.desktop</id><name>Zen Browser</name><releases><release version="1.2.3"/></releases></component>"#,
        )
        .unwrap();
        fs::write(
            runtime_metainfo.join("org.freedesktop.Platform.metainfo.xml"),
            r#"<component><id>org.freedesktop.Platform</id><name>Freedesktop Platform</name><releases><release version="24.08.20"/></releases></component>"#,
        )
        .unwrap();
        fs::write(
            refs.join("apps/app.zen_browser.zen.ini"),
            "origin=flathub\nruntime_origin=flathub\napp_id=app.zen_browser.zen\napp_ref=app/app.zen_browser.zen/x86_64/stable\napp_commit=app-commit\ninstalled_size=402500000\napp_dir=apps/app.zen_browser.zen/app-commit\narch=x86_64\nbranch=stable\nruntime_ref=org.freedesktop.Platform/x86_64/24.08\nruntime_commit=runtime-commit\nruntime_dir=runtimes/flathub/platform/runtime-commit\ncommand=zen\n",
        )
        .unwrap();
        fs::write(
            refs.join("runtimes/flathub--org.freedesktop.Platform_x86_64_24.08.ini"),
            "origin=flathub\nruntime_ref=org.freedesktop.Platform/x86_64/24.08\nruntime_commit=runtime-commit\ninstalled_size=659900000\nruntime_dir=runtimes/flathub/platform/runtime-commit\n",
        )
        .unwrap();
        let extension = data.join("extensions/org.freedesktop.Platform.GL.default-24.08");
        fs::create_dir_all(extension.join("files")).unwrap();
        let extension_metainfo = extension.join("files/share/metainfo");
        fs::create_dir_all(&extension_metainfo).unwrap();
        fs::write(
            extension_metainfo.join("org.freedesktop.Platform.GL.default.metainfo.xml"),
            r#"<component><id>org.freedesktop.Platform.GL.default</id><name>Default GL</name><releases><release version="24.08"/></releases></component>"#,
        )
        .unwrap();
        fs::write(extension.join("metadata"), "[Runtime]\n").unwrap();
        fs::write(
            extension.join(".ostree-commit"),
            "runtime/org.freedesktop.Platform.GL.default/x86_64/24.08\nextension-commit\n457000000\nflathub\n",
        )
        .unwrap();
        Self {
            root,
            data,
            app_location,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_flatpak"))
            .args(args)
            .env("HOME", self.root.join("home"))
            .env("FREEBSD_FLATPAK_DATA_DIR", &self.data)
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

fn stdout(output: Output) -> String {
    assert!(output.status.success(), "command failed: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn row_ids(output: &str) -> Vec<&str> {
    output
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .collect()
}

#[test]
fn list_columns_and_kind_filters_include_runtime_extensions() {
    let fixture = Fixture::new();
    assert_eq!(
        stdout(fixture.run(&["list", "--app"])),
        concat!(
            "Name           Application ID         Version    Branch    Origin\n",
            "Zen Browser    app.zen_browser.zen    1.2.3      stable    flathub\n",
        )
    );
    let all = stdout(fixture.run(&["list", "--columns=application,size"]));
    assert!(all.starts_with("Application ID"));
    assert!(all.contains("app.zen_browser.zen"));
    assert!(all.contains("402.5 MB"));
    assert!(all.contains("org.freedesktop.Platform"));
    assert!(all.contains("659.9 MB"));
    assert!(all.contains("org.freedesktop.Platform.GL.default"));
    assert!(all.contains("457.0 MB"));

    let apps = stdout(fixture.run(&["list", "--app", "--columns=application,size"]));
    assert_eq!(row_ids(&apps), ["app.zen_browser.zen"]);

    let runtime_defaults = stdout(fixture.run(&["list", "--runtime"]));
    assert!(runtime_defaults.contains("Default GL"));
    assert!(runtime_defaults.contains("24.08"));
    assert!(runtime_defaults.contains("Freedesktop Platform"));
    assert!(runtime_defaults.contains("24.08.20"));

    let runtimes = stdout(fixture.run(&["list", "--runtime", "--columns=application,size"]));
    assert_eq!(
        row_ids(&runtimes),
        [
            "org.freedesktop.Platform.GL.default",
            "org.freedesktop.Platform"
        ]
    );
}

#[test]
fn details_and_column_validation_behave_like_installed_ref_options() {
    let fixture = Fixture::new();
    let details = stdout(fixture.run(&["list", "--show-details"]));
    for header in [
        "Application ID",
        "Arch",
        "Branch",
        "Runtime",
        "Ref",
        "Origin",
        "Installation",
        "Active commit",
        "Installed size",
    ] {
        assert!(details.contains(header), "missing {header}: {details}");
    }

    let invalid = fixture.run(&["list", "--columns=application,imaginary"]);
    assert!(!invalid.status.success());
    assert!(String::from_utf8(invalid.stderr)
        .unwrap()
        .contains("unknown list column"));
}

#[test]
fn info_reports_persisted_size_and_deployment_location() {
    let fixture = Fixture::new();
    assert_eq!(
        stdout(fixture.run(&["info", "--show-size", "app.zen_browser.zen"])),
        "402500000\n"
    );
    assert_eq!(
        stdout(fixture.run(&["info", "--show-location", "app.zen_browser.zen", "stable",])),
        format!("{}\n", fixture.app_location.display())
    );
    let normal = stdout(fixture.run(&["info", "org.freedesktop.Platform", "24.08"]));
    assert!(normal.contains("runtime/org.freedesktop.Platform/x86_64/24.08"));
    assert!(normal.contains("659.9 MB"));
}
