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
