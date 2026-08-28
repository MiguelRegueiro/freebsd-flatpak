use super::error_output::render;
use anyhow::anyhow;

#[test]
fn error_prefix_is_plain_when_styling_is_disabled() {
    assert_eq!(
        render(&anyhow!("not installed"), false),
        "error: not installed"
    );
}

#[test]
fn terminal_styling_colors_only_the_error_prefix() {
    assert_eq!(
        render(&anyhow!("not installed"), true),
        "\x1b[31m\x1b[1merror:\x1b[22m\x1b[0m not installed"
    );
}
