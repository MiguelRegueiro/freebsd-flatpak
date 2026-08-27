use super::*;

#[test]
fn renders_friendly_application_and_runtime_rows() {
    let rows = vec![
        UpdateRow::application("org.gnome.Calculator", "stable", "flathub"),
        UpdateRow::runtime("org.gnome.Platform/x86_64/50", "flathub"),
    ];

    assert_eq!(
        render(&rows),
        concat!(
            "\n        ID                    Branch  Type         Remote\n",
            " 1.     org.gnome.Calculator  stable  Application  flathub\n",
            " 2.     org.gnome.Platform    50      Runtime      flathub\n",
        )
    );
}

#[test]
fn runtime_row_uses_the_resulting_branch() {
    let row = UpdateRow::runtime("org.gnome.Platform/x86_64/50", "flathub");
    let output = render(std::slice::from_ref(&row));

    assert_eq!(row.id, "org.gnome.Platform");
    assert_eq!(row.branch, "50");
    assert_eq!(row.kind, "Runtime");
    assert_eq!(row.remote, "flathub");
    assert!(output.contains("org.gnome.Platform  50"));
    assert!(!output.contains("49"));
}

#[test]
fn normal_rendering_does_not_expose_refs_or_commits() {
    let output = render(&[UpdateRow::application(
        "org.gnome.Calculator",
        "stable",
        "flathub",
    )]);

    assert!(!output.contains("app/"));
    assert!(!output.contains("runtime/"));
    assert!(!output.contains("111111111111"));
}

#[test]
fn verbose_change_shortens_commits() {
    assert_eq!(
        short_change(
            "app/org.example.App/x86_64/stable",
            "app/org.example.App/x86_64/stable",
            "11111111111111111111",
            "22222222222222222222",
        ),
        "app/org.example.App/x86_64/stable, 111111111111 → 222222222222"
    );
}

#[test]
fn verbose_change_shows_a_resulting_ref_change() {
    assert_eq!(
        short_change("runtime/a/x/49", "runtime/a/x/50", "same", "same"),
        "runtime/a/x/49 → runtime/a/x/50 at same (refresh)"
    );
}
