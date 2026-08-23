#!/bin/sh

# Human-facing installer messages use a separate descriptor so callers can
# capture build output without hiding progress. Standalone build-script runs
# fall back to standard output.
ui_heading() {
    [ "${INSTALL_SUPPRESS_PROGRESS:-0}" = 1 ] && return 0
    if [ "${INSTALL_PROGRESS_FD:-}" = 3 ]; then
        printf '\n%s==> %s%s\n' "${INSTALL_HEADING_COLOR:-}" "$1" \
            "${INSTALL_COLOR_RESET:-}" >&3
    else
        printf '\n==> %s\n' "$1"
    fi
}

ui_progress() {
    [ "${INSTALL_SUPPRESS_PROGRESS:-0}" = 1 ] && return 0
    if [ "${INSTALL_PROGRESS_FD:-}" = 3 ]; then
        printf '    %s\n' "$1" >&3
    else
        printf '    %s\n' "$1"
    fi
}
