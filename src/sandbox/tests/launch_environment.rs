use super::*;
use crate::sandbox::launch_application::FlatpakApp;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static TEST_ID: AtomicUsize = AtomicUsize::new(0);

fn test_dir(name: &str) -> PathBuf {
    let id = TEST_ID.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "freebsd-flatpak-env-test-{name}-{}-{id}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}
fn app_with_metadata(metadata: &str) -> FlatpakApp {
    let app_dir = test_dir("app");
    fs::write(app_dir.join("metadata"), metadata).unwrap();
    FlatpakApp {
        app_id: "org.example.App".into(),
        app_dir,
        runtime_ref: "org.freedesktop.Platform/x86_64/25.08".into(),
        runtime_dir: PathBuf::from("/runtime"),
        command: "app".into(),
        args: Vec::new(),
    }
}

fn metadata_for(app: &FlatpakApp) -> String {
    fs::read_to_string(app.app_dir.join("metadata")).unwrap()
}

#[test]
fn app_metadata_environment_expands_existing_sandbox_values() {
    let app = app_with_metadata(
        "[Environment]\nMESA_SHADER_CACHE_DIR=$XDG_RUNTIME_DIR/app/$FLATPAK_ID/cache/mesa_shader_cache_db\nGTK_PATH=/app/lib/gtkmodules\n",
    );
    let base_env = vec![
        ("XDG_RUNTIME_DIR".to_string(), "/run/user/1001".to_string()),
        ("FLATPAK_ID".to_string(), "org.example.App".to_string()),
    ];

    assert_eq!(
        metadata_env("", &metadata_for(&app), &base_env),
        vec![
            (
                "MESA_SHADER_CACHE_DIR".to_string(),
                "/run/user/1001/app/org.example.App/cache/mesa_shader_cache_db".to_string()
            ),
            ("GTK_PATH".to_string(), "/app/lib/gtkmodules".to_string())
        ]
    );
}

#[test]
fn launch_data_dirs_include_projected_host_icon_themes() {
    let app = app_with_metadata("[Application]\nname=org.example.App\n");
    let desktop = DesktopSession {
        xdg_runtime_dir: PathBuf::from("/run/host-user"),
        wayland_display: "wayland-0".into(),
        display: None,
        dbus_session_bus_address: None,
    };

    let env = launch_env(
        &app,
        &desktop,
        1001,
        "user",
        PathBuf::from("/host/data").as_path(),
    );

    assert_eq!(
        env.iter()
            .find(|(key, _)| key == "XDG_DATA_DIRS")
            .map(|(_, value)| value.as_str()),
        Some("/app/share:/usr/share:/usr/share/runtime/share:/run/host/share")
    );
    assert_eq!(
        env.iter()
            .find(|(key, _)| key == "HOST_XDG_DATA_HOME")
            .map(|(_, value)| value.as_str()),
        Some("/host/data")
    );
}

#[test]
fn app_metadata_environment_supports_braced_variables() {
    let app = app_with_metadata("[Environment]\nEXAMPLE=${XDG_RUNTIME_DIR}/app/${FLATPAK_ID}\n");
    let base_env = vec![
        ("XDG_RUNTIME_DIR".to_string(), "/run/user/1001".to_string()),
        ("FLATPAK_ID".to_string(), "org.example.App".to_string()),
    ];

    assert_eq!(
        metadata_env("", &metadata_for(&app), &base_env),
        vec![(
            "EXAMPLE".to_string(),
            "/run/user/1001/app/org.example.App".to_string()
        )]
    );
}

#[test]
fn runtime_metadata_environment_is_propagated() {
    let runtime_metadata =
        "[Environment]\nRUNTIME_ONLY=from-runtime\nSECOND_RUNTIME_VALUE=second\n";

    assert_eq!(
        metadata_env(runtime_metadata, "", &[]),
        vec![
            ("RUNTIME_ONLY".to_string(), "from-runtime".to_string()),
            ("SECOND_RUNTIME_VALUE".to_string(), "second".to_string())
        ]
    );
}

#[test]
fn app_metadata_environment_overrides_runtime_environment() {
    let runtime_metadata = "[Environment]\nSHARED=runtime\nRUNTIME_ONLY=from-runtime\n";
    let app_metadata = "[Environment]\nSHARED=app\nAPP_ONLY=from-app\n";

    assert_eq!(
        metadata_env(runtime_metadata, app_metadata, &[]),
        vec![
            ("SHARED".to_string(), "app".to_string()),
            ("RUNTIME_ONLY".to_string(), "from-runtime".to_string()),
            ("APP_ONLY".to_string(), "from-app".to_string())
        ]
    );
}

#[test]
fn ordinary_apps_keep_graphics_shims_in_ld_preload() {
    let mut env = Vec::new();

    apply_graphics_preloads(
        &mut env,
        vec!["/shim/drm.so".to_string(), "/shim/wayland.so".to_string()],
        vec!["/shim/wayland.so".to_string(), "/shim/drm.so".to_string()],
    );

    assert_eq!(
        env.iter()
            .find(|(key, _)| key == "LD_PRELOAD")
            .map(|(_, value)| value.as_str()),
        Some("/shim/drm.so:/shim/wayland.so")
    );
}

#[test]
fn zypak_gets_both_graphics_shims() {
    let mut env = Vec::new();

    apply_graphics_preloads(
        &mut env,
        vec!["/shim/drm.so".to_string(), "/shim/wayland.so".to_string()],
        vec!["/shim/wayland.so".to_string(), "/shim/drm.so".to_string()],
    );

    assert_eq!(
        env.iter()
            .find(|(key, _)| key == "ZYPAK_LD_PRELOAD")
            .map(|(_, value)| value.as_str()),
        Some("/shim/wayland.so:/shim/drm.so")
    );
}

#[test]
fn zypak_graphics_preloads_preserve_existing_value() {
    let mut env = vec![(
        "ZYPAK_LD_PRELOAD".to_string(),
        "/app/existing.so".to_string(),
    )];

    apply_graphics_preloads(
        &mut env,
        Vec::new(),
        vec!["/shim/wayland.so".to_string(), "/shim/drm.so".to_string()],
    );

    assert_eq!(
        env.iter()
            .find(|(key, _)| key == "ZYPAK_LD_PRELOAD")
            .map(|(_, value)| value.as_str()),
        Some("/shim/wayland.so:/shim/drm.so:/app/existing.so")
    );
}

#[test]
fn normal_and_zypak_preload_values_remain_independent() {
    let mut env = vec![
        ("LD_PRELOAD".to_string(), "/app/normal.so".to_string()),
        ("ZYPAK_LD_PRELOAD".to_string(), "/app/zypak.so".to_string()),
    ];

    apply_graphics_preloads(
        &mut env,
        vec!["/shim/drm.so".to_string()],
        vec!["/shim/wayland.so".to_string()],
    );

    assert_eq!(
        env.iter()
            .find(|(key, _)| key == "LD_PRELOAD")
            .map(|(_, value)| value.as_str()),
        Some("/shim/drm.so:/app/normal.so")
    );
    assert_eq!(
        env.iter()
            .find(|(key, _)| key == "ZYPAK_LD_PRELOAD")
            .map(|(_, value)| value.as_str()),
        Some("/shim/wayland.so:/app/zypak.so")
    );
}

#[test]
fn metadata_runtime_dirs_are_created_inside_app_runtime_scope() {
    let dir = test_dir("metadata-runtime-dirs");
    let host_runtime = dir.join("xdg-runtime");
    fs::create_dir_all(&host_runtime).unwrap();
    let env = vec![
        (
            "MESA_SHADER_CACHE_DIR".to_string(),
            "/run/user/1001/app/org.example.App/cache/mesa_shader_cache_db".to_string(),
        ),
        (
            "OTHER_DIR".to_string(),
            "/run/user/1001/other/cache".to_string(),
        ),
    ];

    ensure_metadata_runtime_dirs(&env, &host_runtime, 1001, "org.example.App").unwrap();

    assert!(host_runtime
        .join("app/org.example.App/cache/mesa_shader_cache_db")
        .is_dir());
    assert!(!host_runtime.join("other/cache").exists());
}
