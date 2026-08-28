use std::io::IsTerminal;

const BOLD_ON: &str = "\x1b[1m";
const BOLD_OFF: &str = "\x1b[22m";
const RED: &str = "\x1b[31m";
const COLOR_RESET: &str = "\x1b[0m";

pub(super) fn stdout_enabled() -> bool {
    std::io::stdout().is_terminal()
}

pub(super) fn stderr_enabled() -> bool {
    std::io::stderr().is_terminal()
}

pub(super) fn bold(value: &str, enabled: bool) -> String {
    if enabled {
        format!("{BOLD_ON}{value}{COLOR_RESET}")
    } else {
        value.to_string()
    }
}

pub(super) fn error_label(enabled: bool) -> String {
    if enabled {
        format!("{RED}{BOLD_ON}error:{BOLD_OFF}{COLOR_RESET}")
    } else {
        "error:".to_string()
    }
}
