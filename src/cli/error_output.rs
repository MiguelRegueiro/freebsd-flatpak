use std::io::IsTerminal;

const RED: &str = "\x1b[31m";
const BOLD_ON: &str = "\x1b[1m";
const BOLD_OFF: &str = "\x1b[22m";
const COLOR_RESET: &str = "\x1b[0m";

pub(crate) fn report(error: &anyhow::Error) {
    eprintln!("{}", render(error, std::io::stderr().is_terminal()));
}

pub(super) fn render(error: &anyhow::Error, styled: bool) -> String {
    if styled {
        format!("{RED}{BOLD_ON}error:{BOLD_OFF}{COLOR_RESET} {error:#}")
    } else {
        format!("error: {error:#}")
    }
}
