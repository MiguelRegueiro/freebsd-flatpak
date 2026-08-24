use super::*;
use std::path::Path;

#[test]
fn rewrites_simple_exec() {
    assert_eq!(
        desktop_exec(
            Path::new("/project/bin/flatpak"),
            "org.example.App",
            "app-binary"
        ),
        "/project/bin/flatpak run org.example.App"
    );
}

#[test]
fn preserves_desktop_exec_arguments() {
    assert_eq!(
        desktop_exec(
            Path::new("/project/bin/flatpak"),
            "org.example.App",
            "app-binary --new-window %U"
        ),
        "/project/bin/flatpak run org.example.App -- --new-window %U"
    );
}

#[test]
fn handles_quoted_exec_command() {
    assert_eq!(
        exec_tail_after_command("\"/app/bin/example command\" %F"),
        "%F"
    );
}
