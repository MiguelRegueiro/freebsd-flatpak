pub(crate) fn report(error: &anyhow::Error) {
    eprintln!("{}", render(error, super::style::stderr_enabled()));
}

pub(super) fn render(error: &anyhow::Error, styled: bool) -> String {
    format!("{} {error:#}", super::style::error_label(styled))
}
