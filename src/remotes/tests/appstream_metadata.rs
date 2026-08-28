use super::{parse_appstream_info, parse_appstream_replacements};

#[test]
fn appstream_replacements_map_old_ids_to_current_component() {
    let xml = r#"
<components>
  <component type="desktop-application">
    <id>app.example.Current</id>
    <name>Example</name>
    <replaces>
      <id>org.example.Old</id>
      <id>org.example.Older</id>
    </replaces>
  </component>
</components>
"#;

    let replacements = parse_appstream_replacements(xml);

    assert_eq!(
        replacements.get("org.example.Old").unwrap(),
        &vec!["app.example.Current".to_string()]
    );
    assert_eq!(
        replacements.get("org.example.Older").unwrap(),
        &vec!["app.example.Current".to_string()]
    );
}

#[test]
fn appstream_info_reads_display_fields_and_latest_version() {
    let xml = r#"
<components>
  <component type="desktop-application">
    <id>org.example.App</id>
    <name>Example &amp; More</name>
    <summary>Do useful things</summary>
    <project_license>GPL-3.0-or-later</project_license>
    <releases>
      <release version="50.0" date="2026-01-01"/>
      <release version="49.0" date="2025-01-01"/>
    </releases>
  </component>
</components>
"#;

    let info = parse_appstream_info(xml, "org.example.App").unwrap();

    assert_eq!(info.name.as_deref(), Some("Example & More"));
    assert_eq!(info.summary.as_deref(), Some("Do useful things"));
    assert_eq!(info.version.as_deref(), Some("50.0"));
    assert_eq!(info.license.as_deref(), Some("GPL-3.0-or-later"));
}

#[test]
fn appstream_info_preserves_missing_optional_fields() {
    let xml = r#"
<components>
  <component type="desktop-application">
    <id>org.example.App</id>
    <name>Example</name>
  </component>
</components>
"#;

    let info = parse_appstream_info(xml, "org.example.App").unwrap();

    assert_eq!(info.name.as_deref(), Some("Example"));
    assert_eq!(info.summary, None);
    assert_eq!(info.version, None);
    assert_eq!(info.license, None);
    assert!(parse_appstream_info(xml, "org.example.Missing").is_none());
}

#[test]
fn appstream_info_matches_desktop_component_suffix() {
    let xml = r#"<component><id>org.example.App.desktop</id><name>Example</name></component>"#;

    assert_eq!(
        parse_appstream_info(xml, "org.example.App")
            .unwrap()
            .name
            .as_deref(),
        Some("Example")
    );
}

#[test]
fn appstream_info_uses_component_name_instead_of_nested_developer_name() {
    let xml = r#"
<component type="desktop-application">
  <id>org.gnome.TextEditor</id>
  <developer id="org.gnome">
    <name>The GNOME Project</name>
  </developer>
  <name>Text Editor</name>
  <releases>
    <release version="50.1"/>
  </releases>
</component>
"#;

    let info = parse_appstream_info(xml, "org.gnome.TextEditor").unwrap();

    assert_eq!(info.name.as_deref(), Some("Text Editor"));
    assert_eq!(info.version.as_deref(), Some("50.1"));
}
