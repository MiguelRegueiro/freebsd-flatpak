use super::*;

#[test]
fn parses_cursor_vars_from_procstat_text() {
    let env = parse_environment_tokens(
            "PID COMM ENVIRONMENT\n1 Hyprland XCURSOR_THEME=Bibata XCURSOR_SIZE=24 HYPRCURSOR_THEME=Bibata HYPRCURSOR_SIZE=24 PATH=/bin\n",
        );
    assert_eq!(env.get("XCURSOR_THEME").map(String::as_str), Some("Bibata"));
    assert_eq!(env.get("HYPRCURSOR_SIZE").map(String::as_str), Some("24"));
    assert!(!env.contains_key("PATH"));
}

#[test]
fn validates_theme_names_as_single_path_component() {
    assert!(valid_theme_name("Bibata-Modern-Classic"));
    assert!(!valid_theme_name("../Bibata"));
    assert!(!valid_theme_name("parent/child"));
    assert!(!valid_theme_name(""));
}

#[test]
fn parses_gsettings_icon_theme_string() {
    assert_eq!(
        parse_gsettings_string("'MacTahoe-dark'\n").as_deref(),
        Some("MacTahoe-dark")
    );
    assert_eq!(parse_gsettings_string("@ms nothing"), None);
    assert_eq!(
        parse_quoted_variant_string("(<<'MacTahoe-dark'>>,)\n").as_deref(),
        Some("MacTahoe-dark")
    );
}

#[test]
fn mounts_non_runtime_icon_theme_and_uses_runtime_fallbacks() {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-icon-theme-test-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let theme = root.join("Example");
    fs::create_dir_all(&theme).unwrap();
    fs::write(
        theme.join("index.theme"),
        "[Icon Theme]\nName=Example\nInherits=Adwaita,hicolor\n",
    )
    .unwrap();
    let themes = BTreeSet::from(["Example".to_string()]);
    let mut warnings = Vec::new();

    let mounts = theme_mounts(&themes, std::slice::from_ref(&root), &mut warnings);

    assert!(warnings.is_empty());
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0].theme, "Example");
    assert_eq!(
        mounts[0].sandbox_path,
        PathBuf::from("/run/host/share/icons/Example")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn prepares_standard_gtk_cursor_settings_for_x11_toolkits() {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-cursor-settings-test-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let cursor = HostCursorTheme {
        icon_theme: None,
        gtk_theme: None,
        xcursor_theme: Some("Bibata-Modern-Classic".to_string()),
        xcursor_size: Some("24".to_string()),
        hyprcursor_theme: None,
        hyprcursor_size: None,
        mounts: Vec::new(),
        warnings: Vec::new(),
    };

    cursor.prepare(&root).unwrap();

    let expected =
        "[Settings]\ngtk-cursor-theme-name=Bibata-Modern-Classic\ngtk-cursor-theme-size=24\n";
    for version in ["gtk-3.0", "gtk-4.0"] {
        assert_eq!(
            fs::read_to_string(
                root.join("run/host/freebsd-flatpak-cursor-config")
                    .join(version)
                    .join("settings.ini")
            )
            .unwrap(),
            expected
        );
    }
    assert_eq!(
        cursor.config_dirs(),
        vec![SANDBOX_CURSOR_CONFIG_ROOT.to_string()]
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn unsafe_cursor_theme_does_not_create_toolkit_settings() {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-unsafe-cursor-settings-test-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let cursor = HostCursorTheme {
        icon_theme: None,
        gtk_theme: None,
        xcursor_theme: Some("../escape".to_string()),
        xcursor_size: Some("24".to_string()),
        hyprcursor_theme: None,
        hyprcursor_size: None,
        mounts: Vec::new(),
        warnings: Vec::new(),
    };

    cursor.prepare(&root).unwrap();

    assert!(cursor.config_dirs().is_empty());
    assert!(!root.join("run/host/freebsd-flatpak-cursor-config").exists());
    let _ = fs::remove_dir_all(root);
}
