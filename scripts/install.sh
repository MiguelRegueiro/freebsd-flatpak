#!/bin/sh
set -eu

usage() {
    cat <<EOF
Usage: $0 [OPTIONS]

Build and install FreeBSD Flatpak.

Options:
  -v, --verbose  Show complete build output live (also saved to the log)
  -n, --dry-run  Show the planned work without building or installing
  -h, --help     Show this help

Short options may be combined; use -nv or -vn for a verbose dry run.
EOF
}

VERBOSE=0
DRY_RUN=0
while [ "$#" -gt 0 ]; do
    case $1 in
        -v|--verbose) VERBOSE=1 ;;
        -n|--dry-run) DRY_RUN=1 ;;
        -h|--help)
            usage
            exit 0
            ;;
        --)
            shift
            break
            ;;
        --*)
            printf 'Error: unknown option: %s\n\n' "$1" >&2
            usage >&2
            exit 64
            ;;
        -?*)
            short_options=${1#-}
            while [ -n "$short_options" ]; do
                short_option=${short_options%"${short_options#?}"}
                short_options=${short_options#?}
                case $short_option in
                    v) VERBOSE=1 ;;
                    n) DRY_RUN=1 ;;
                    h)
                        usage
                        exit 0
                        ;;
                    *)
                        printf 'Error: unknown option: -%s\n\n' "$short_option" >&2
                        usage >&2
                        exit 64
                        ;;
                esac
            done
            ;;
        *)
            printf 'Error: unexpected argument: %s\n\n' "$1" >&2
            usage >&2
            exit 64
            ;;
    esac
    shift
done
if [ "$#" -gt 0 ]; then
    printf 'Error: unexpected argument: %s\n\n' "$1" >&2
    usage >&2
    exit 64
fi

BASE=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd) || {
    printf 'Error: could not locate the source directory\n' >&2
    exit 1
}
. "$BASE/scripts/install-ui.sh"

if [ -t 1 ]; then
    INSTALL_HEADING_COLOR=$(printf '\033[1;34m')
    INSTALL_COLOR_RESET=$(printf '\033[0m')
else
    INSTALL_HEADING_COLOR=
    INSTALL_COLOR_RESET=
fi
export INSTALL_HEADING_COLOR INSTALL_COLOR_RESET
INSTALL_PROGRESS_FD=3
export INSTALL_PROGRESS_FD
exec 3>&1

LOG_FILE=
STEP_LOG=
OUTPUT_FIFO=
SPINNER_PID=
PHASE_START=
PHASE_WARNINGS=
PHASE_LIVE_OUTPUT=0
PHASE_NATIVE_TTY=0
PHASE_SHOW_WARNINGS=1
ACTIVE_PID=
ACTIVE_PROCESS_GROUP=0
TEE_PID=
NATIVE_INPUT_FIFO=
NATIVE_INPUT_OPEN=0
NATIVE_READY_FIFO=
CARGO_TERMINAL=
CARGO_TERMINAL_STATE=
CARGO_PTY=
CURSOR_HIDDEN=0

cursor_hide() {
    if [ -t 1 ]; then
        printf '\033[?25l' >&3
        CURSOR_HIDDEN=1
    fi
}

cursor_restore() {
    if [ "$CURSOR_HIDDEN" -eq 1 ]; then
        printf '\033[?25h' >&3
        CURSOR_HIDDEN=0
    fi
}

spinner_stop() {
    if [ -n "$SPINNER_PID" ]; then
        kill "$SPINNER_PID" >/dev/null 2>&1 || true
        wait "$SPINNER_PID" >/dev/null 2>&1 || true
        SPINNER_PID=
        printf '\r\033[2K' >&3
    fi
}

stop_active_process() {
    if [ -n "$ACTIVE_PID" ]; then
        if [ "$ACTIVE_PROCESS_GROUP" -eq 1 ]; then
            kill -TERM "-$ACTIVE_PID" >/dev/null 2>&1 ||
                kill -TERM "$ACTIVE_PID" >/dev/null 2>&1 || true
        else
            kill -TERM "$ACTIVE_PID" >/dev/null 2>&1 || true
        fi
        sleep 0.1
        if [ "$ACTIVE_PROCESS_GROUP" -eq 1 ] &&
            kill -0 "-$ACTIVE_PID" >/dev/null 2>&1; then
            kill -KILL "-$ACTIVE_PID" >/dev/null 2>&1 || true
        elif kill -0 "$ACTIVE_PID" >/dev/null 2>&1; then
            kill -KILL "$ACTIVE_PID" >/dev/null 2>&1 || true
        fi
        wait "$ACTIVE_PID" >/dev/null 2>&1 || true
        ACTIVE_PID=
        ACTIVE_PROCESS_GROUP=0
    fi
    if [ -n "$TEE_PID" ]; then
        kill "$TEE_PID" >/dev/null 2>&1 || true
        wait "$TEE_PID" >/dev/null 2>&1 || true
        TEE_PID=
    fi
}

close_native_input() {
    if [ "$NATIVE_INPUT_OPEN" -eq 1 ]; then
        exec 4>&-
        NATIVE_INPUT_OPEN=0
    fi
    if [ -n "$NATIVE_INPUT_FIFO" ]; then
        rm -f "$NATIVE_INPUT_FIFO"
        NATIVE_INPUT_FIFO=
    fi
    if [ -n "$NATIVE_READY_FIFO" ]; then
        rm -f "$NATIVE_READY_FIFO"
        NATIVE_READY_FIFO=
    fi
}

sync_cargo_terminal_size() {
    [ -n "$CARGO_PTY" ] || return 0
    outer_size=$(stty -f "$CARGO_TERMINAL" size 2>/dev/null) || return 0
    set -- $outer_size
    [ "$#" -eq 2 ] || return 0
    stty -f "$CARGO_PTY" rows "$1" cols "$2" 2>/dev/null || true
}

restore_cargo_terminal() {
    if [ -n "$CARGO_TERMINAL_STATE" ]; then
        stty -f "$CARGO_TERMINAL" "$CARGO_TERMINAL_STATE" 2>/dev/null || true
        CARGO_TERMINAL=
        CARGO_TERMINAL_STATE=
    fi
}

cleanup() {
    spinner_stop
    stop_active_process
    close_native_input
    restore_cargo_terminal
    [ -z "$OUTPUT_FIFO" ] || rm -f "$OUTPUT_FIFO"
    [ -z "$STEP_LOG" ] || rm -f "$STEP_LOG"
    [ -z "$PHASE_WARNINGS" ] || rm -f "$PHASE_WARNINGS"
}

on_exit() {
    exit_status=$?
    trap - 0
    cleanup
    cursor_restore
    exit "$exit_status"
}

handle_signal() {
    signal_name=$1
    signal_status=$2
    trap '' HUP INT TERM
    spinner_stop
    stop_active_process
    restore_cargo_terminal
    printf '\nError: interrupted by %s\n' "$signal_name" >&2
    if [ -n "$LOG_FILE" ]; then
        printf 'Full log: %s\n' "$LOG_FILE" >&2
    fi
    exit "$signal_status"
}

trap on_exit 0
trap 'handle_signal HUP 129' HUP
trap 'handle_signal Ctrl+C 130' INT
trap 'handle_signal TERM 143' TERM
trap sync_cargo_terminal_size WINCH

fail() {
    spinner_stop
    printf '\nError: %s\n' "$*" >&2
    if [ -n "$LOG_FILE" ]; then
        printf 'Full log: %s\n' "$LOG_FILE" >&2
    fi
    exit 1
}

relevant_warnings() {
    awk '
        {
            lower = tolower($0)
            if (lower ~ /(^|[[:space:]])warning:/ &&
                lower !~ /^[[:space:]]*configure: warning:/) {
                print
                found = 1
            }
        }
        END { exit found ? 0 : 1 }
    ' "$STEP_LOG"
}

collect_warnings() {
    [ "$VERBOSE" -eq 0 ] || return 0
    [ "$PHASE_LIVE_OUTPUT" -eq 0 ] || return 0
    [ "$PHASE_SHOW_WARNINGS" -eq 1 ] || return 0
    if [ -n "$PHASE_WARNINGS" ]; then
        relevant_warnings >>"$PHASE_WARNINGS" 2>/dev/null || true
    elif relevant_warnings >/dev/null 2>&1; then
        relevant_warnings | sed 's/^/        /' >&3
    fi
}

phase_start() {
    ui_heading "$1"
    spinner_status=$2
    PHASE_LIVE_OUTPUT=${3:-0}
    PHASE_NATIVE_TTY=${4:-0}
    PHASE_SHOW_WARNINGS=${5:-1}
    PHASE_START=$(date +%s)
    PHASE_WARNINGS=$(mktemp "${TMPDIR:-/tmp}/freebsd-flatpak-warnings.XXXXXX") ||
        fail "could not create a temporary warning log"

    if [ "$PHASE_NATIVE_TTY" -eq 1 ] && [ -t 1 ]; then
        cursor_hide
    elif [ "$VERBOSE" -eq 0 ] && [ "$PHASE_LIVE_OUTPUT" -eq 0 ] && [ -t 1 ]; then
        cursor_hide
        (
            spinner_frame=0
            spinner_tick=0
            while :; do
                case $spinner_frame in
                    0) spinner_char='⠋' ;;
                    1) spinner_char='⠙' ;;
                    2) spinner_char='⠹' ;;
                    3) spinner_char='⠸' ;;
                    4) spinner_char='⠼' ;;
                    5) spinner_char='⠴' ;;
                    6) spinner_char='⠦' ;;
                    7) spinner_char='⠧' ;;
                    8) spinner_char='⠇' ;;
                    *) spinner_char='⠏' ;;
                esac
                spinner_elapsed=$((spinner_tick / 10))
                printf '\r\033[2K    %s %s %ds' \
                    "$spinner_char" "$spinner_status" "$spinner_elapsed" >&3
                spinner_frame=$(((spinner_frame + 1) % 10))
                spinner_tick=$((spinner_tick + 1))
                sleep 0.1
            done
        ) &
        SPINNER_PID=$!
    fi
}

phase_finish() {
    show_completion=${1:-1}
    spinner_stop
    if [ "$VERBOSE" -eq 0 ]; then
        if [ "$show_completion" -eq 1 ]; then
            phase_end=$(date +%s)
            phase_elapsed=$((phase_end - PHASE_START))
            if [ "$phase_elapsed" -eq 0 ]; then
                phase_duration='<1s'
            else
                phase_duration=${phase_elapsed}s
            fi
            ui_progress "✓ Completed in $phase_duration"
        fi
        if [ -s "$PHASE_WARNINGS" ]; then
            sed 's/^/        /' "$PHASE_WARNINGS" >&3
        fi
    fi
    rm -f "$PHASE_WARNINGS"
    PHASE_WARNINGS=
    if [ "$VERBOSE" -eq 1 ] && [ "$PHASE_NATIVE_TTY" -eq 1 ]; then
        cursor_restore
    fi
}

run_logged() {
    step_name=$1
    shift
    STEP_LOG=$(mktemp "${TMPDIR:-/tmp}/freebsd-flatpak-step.XXXXXX") ||
        fail "could not create a temporary step log"
    printf '\n===== %s =====\n' "$step_name" >>"$LOG_FILE"

    if [ "$PHASE_NATIVE_TTY" -eq 1 ] && [ -t 1 ]; then
        cargo_terminal=$(tty)
        CARGO_TERMINAL=$cargo_terminal
        CARGO_TERMINAL_STATE=$(stty -f "$CARGO_TERMINAL" -g) ||
            fail "could not read the terminal state"
        stty -f "$CARGO_TERMINAL" -onlcr ||
            fail "could not prepare native Cargo output"
        NATIVE_INPUT_FIFO=$STEP_LOG.input
        NATIVE_READY_FIFO=$STEP_LOG.ready
        mkfifo "$NATIVE_INPUT_FIFO" || fail "could not create the Cargo input pipe"
        mkfifo "$NATIVE_READY_FIFO" || fail "could not create the Cargo readiness pipe"
        set -m
        script -q -F -w "$STEP_LOG" /bin/sh -c '
            outer_terminal=$1
            ready_fifo=$2
            shift 2
            outer_size=$(stty -f "$outer_terminal" size 2>/dev/null) || outer_size=
            terminal_rows=${outer_size% *}
            terminal_cols=${outer_size#* }
            if [ -n "$outer_size" ] && [ "$terminal_rows" != "$terminal_cols" ]; then
                stty rows "$terminal_rows" cols "$terminal_cols" </dev/tty
            fi
            tty >"$ready_fifo"
            exec "$@" </dev/tty
        ' cargo-terminal-wrapper "$cargo_terminal" "$NATIVE_READY_FIFO" "$@" \
            <"$NATIVE_INPUT_FIFO" &
        ACTIVE_PID=$!
        ACTIVE_PROCESS_GROUP=1
        set +m
        exec 4>"$NATIVE_INPUT_FIFO"
        NATIVE_INPUT_OPEN=1
        IFS= read -r CARGO_PTY <"$NATIVE_READY_FIFO"
        rm -f "$NATIVE_READY_FIFO"
        NATIVE_READY_FIFO=
        if wait "$ACTIVE_PID"; then
            step_status=0
        else
            step_status=$?
        fi
        ACTIVE_PID=
        ACTIVE_PROCESS_GROUP=0
        close_native_input
        CARGO_PTY=
        restore_cargo_terminal
        cat "$STEP_LOG" >>"$LOG_FILE"
    elif [ "$VERBOSE" -eq 1 ] || [ "$PHASE_LIVE_OUTPUT" -eq 1 ]; then
        OUTPUT_FIFO=$STEP_LOG.fifo
        mkfifo "$OUTPUT_FIFO" || fail "could not create the verbose-output pipe"
        tee -a "$LOG_FILE" "$STEP_LOG" <"$OUTPUT_FIFO" &
        TEE_PID=$!
        set -m
        "$@" >"$OUTPUT_FIFO" 2>&1 &
        ACTIVE_PID=$!
        ACTIVE_PROCESS_GROUP=1
        set +m
        if wait "$ACTIVE_PID"; then
            step_status=0
        else
            step_status=$?
        fi
        ACTIVE_PID=
        ACTIVE_PROCESS_GROUP=0
        wait "$TEE_PID" || true
        TEE_PID=
        rm -f "$OUTPUT_FIFO"
        OUTPUT_FIFO=
    else
        set -m
        "$@" >"$STEP_LOG" 2>&1 &
        ACTIVE_PID=$!
        ACTIVE_PROCESS_GROUP=1
        set +m
        if wait "$ACTIVE_PID"; then
            step_status=0
        else
            step_status=$?
        fi
        ACTIVE_PID=
        ACTIVE_PROCESS_GROUP=0
        cat "$STEP_LOG" >>"$LOG_FILE"
    fi

    if [ "$step_status" -ne 0 ]; then
        spinner_stop
        printf '\nError: %s failed (exit status %s)\n' "$step_name" "$step_status" >&2
        printf '%s\n' '--- last 30 lines of diagnostic output ---' >&2
        tail -n 30 "$STEP_LOG" >&2
        printf '%s\n' '------------------------------------------' >&2
        printf 'Full log: %s\n' "$LOG_FILE" >&2
        exit "$step_status"
    fi
    collect_warnings
    rm -f "$STEP_LOG"
    STEP_LOG=
}

INSTALL_BIN=/usr/local/bin
INSTALL_LIBEXEC=/usr/local/libexec/freebsd-flatpak
INSTALL_LICENSES=/usr/local/share/licenses/freebsd-flatpak
HELPER_BUILD_DIR=target/release/freebsd-flatpak-helpers
OSTREE_PREFIX="$BASE/target/vendor-ostree/prefix"
LINUX_CC=${LINUX_CC:-/compat/linux/usr/bin/gcc}
LINUX_BUILD_PATH=/compat/linux/usr/bin:/compat/linux/bin:/usr/bin:/bin
REQUIRED_PACKAGES="rust gmake pkgconf glib gdk-pixbuf2 curl gpgme pipewire"

print_install_tree() {
    install_root=${INSTALL_BIN%/*}
    install_bin_branch=${INSTALL_BIN#"$install_root"/}
    install_libexec_branch=${INSTALL_LIBEXEC#"$install_root"/}
    install_licenses_branch=${INSTALL_LICENSES#"$install_root"/}

    printf '    %s\n' "$install_root"
    printf '%s\n' \
        "    ├── $install_bin_branch" \
        '    │   └── flatpak' \
        "    ├── $install_libexec_branch" \
        '    │   ├── libostree-1.so.1' \
        '    │   ├── portal-bridge' \
        '    │   ├── status-notifier-bridge' \
        '    │   ├── libwayland-drm-devt-shim.so' \
        '    │   ├── libdrm-syncobj-errno-shim.so' \
        '    │   └── libchromium-zygote-drm-preload.so' \
        "    └── $install_licenses_branch" \
        '        ├── BSD-2-Clause.txt' \
        '        ├── LGPL-2.0-or-later.txt' \
        '        └── MIT-ostree-rs.txt' >&3
}

dry_command() {
    [ "$VERBOSE" -eq 1 ] || return 0
    printf '        $ %s\n' "$1" >&3
}

dry_note() {
    [ "$VERBOSE" -eq 1 ] || return 0
    printf '        # %s\n' "$1" >&3
}

if [ "$DRY_RUN" -eq 1 ]; then
    BUILD_USER=${DOAS_USER:-${SUDO_USER:-$(id -un)}}
    ui_heading "Dry run (no build or install commands will be executed)"
    ui_progress "Build as $BUILD_USER"
    ui_progress "Check packages: $REQUIRED_PACKAGES"
    if [ "$VERBOSE" -eq 0 ]; then
        ui_heading "Building private libostree 2026.3"
        ui_heading "Building FreeBSD Flatpak"
        ui_heading "Building compatibility helpers"
        ui_heading "Installing FreeBSD Flatpak"
        print_install_tree
        ui_heading "Installation complete (dry run)"
        exit 0
    fi
    for package_name in $REQUIRED_PACKAGES; do
        dry_command "pkg info -e '$package_name'"
    done
    dry_note "Run pkg install -y with only the package names found missing."
    ui_heading "Building private libostree 2026.3"
    ui_progress "Download and prepare source if needed"
    dry_command "su '$BUILD_USER' -c './scripts/build-libostree.sh'"
    dry_note "The build script fetches, verifies, extracts, and patches only when needed."
    ui_progress "Configuring"
    dry_command "'$BASE/target/vendor-ostree/src/libostree-2026.3/configure' --disable-maintainer-mode --prefix='$OSTREE_PREFIX' --with-curl --without-soup --without-soup3 --with-gpgme --without-composefs --without-selinux --without-avahi --without-libmount --without-libsystemd --without-libarchive --disable-rofiles-fuse --disable-introspection --disable-man --disable-gtk-doc --disable-otmpfile"
    ui_progress "Building"
    dry_command "gmake libglnx-config.h src/libostree/ostree-enumtypes.c"
    dry_command 'gmake -j "$(sysctl -n hw.ncpu 2>/dev/null || echo 2)" libostree-1.la'
    ui_progress "Staging"
    dry_command "gmake install-libLTLIBRARIES install-pkgconfigDATA"
    ui_heading "Building FreeBSD Flatpak"
    ui_progress "Rust CLI"
    dry_command "su '$BUILD_USER' -c \"env PKG_CONFIG_PATH='$OSTREE_PREFIX/lib/pkgconfig' LIBRARY_PATH='$OSTREE_PREFIX/lib' cargo build --locked --release --bin flatpak\""
    ui_heading "Building compatibility helpers"
    ui_progress "Portal bridge"
    dry_command "su '$BUILD_USER' -c \"/bin/sh ./scripts/build-compatibility-bridges.sh '$HELPER_BUILD_DIR'\""
    ui_progress "Status notifier bridge"
    ui_progress "Compatibility shims"
    dry_command "su '$BUILD_USER' -c \"env PATH='$LINUX_BUILD_PATH' '$LINUX_CC' -shared -fPIC -O2 -Wall -Wextra compatibility_helpers/wayland-drm-devt-shim.c -o '$HELPER_BUILD_DIR/libwayland-drm-devt-shim.so' -ldl\""
    dry_command "su '$BUILD_USER' -c \"env PATH='$LINUX_BUILD_PATH' '$LINUX_CC' -shared -fPIC -O2 -Wall -Wextra compatibility_helpers/drm-syncobj-errno-shim.c -o '$HELPER_BUILD_DIR/libdrm-syncobj-errno-shim.so' -ldl -pthread\""
    dry_command "su '$BUILD_USER' -c \"env PATH='$LINUX_BUILD_PATH' '$LINUX_CC' -shared -fPIC -O2 -Wall -Wextra compatibility_helpers/chromium-zygote-drm-preload.c -o '$HELPER_BUILD_DIR/libchromium-zygote-drm-preload.so' -ldl -pthread\""
    ui_heading "Installing FreeBSD Flatpak"
    print_install_tree
    dry_command "install -d -o root -g wheel -m 755 '$INSTALL_BIN' '$INSTALL_LIBEXEC' '$INSTALL_LICENSES'"
    dry_command "install -o root -g wheel -m 755 target/release/flatpak '$INSTALL_BIN/flatpak'"
    dry_command "install -o root -g wheel -m 755 '$OSTREE_PREFIX/lib/libostree-1.so.1.0.0' '$INSTALL_LIBEXEC/libostree-1.so.1'"
    dry_command "install -o root -g wheel -m 644 LICENSE '$INSTALL_LICENSES/BSD-2-Clause.txt'"
    dry_command "install -o root -g wheel -m 644 LICENSES/LGPL-2.0-or-later.txt LICENSES/MIT-ostree-rs.txt '$INSTALL_LICENSES/'"
    dry_command "install -o root -g wheel -m 755 '$HELPER_BUILD_DIR/portal-bridge' '$HELPER_BUILD_DIR/status-notifier-bridge' '$HELPER_BUILD_DIR/libwayland-drm-devt-shim.so' '$HELPER_BUILD_DIR/libdrm-syncobj-errno-shim.so' '$HELPER_BUILD_DIR/libchromium-zygote-drm-preload.so' '$INSTALL_LIBEXEC/'"
    dry_command "ldd '$INSTALL_BIN/flatpak'  # verify shared-library dependencies"
    ui_heading "Installation complete (dry run)"
    exit 0
fi

if [ "$(id -u)" -ne 0 ]; then
    fail "this installer must run as root; rerun it with 'doas ./scripts/install.sh' or 'sudo ./scripts/install.sh'"
fi

command -v pkg >/dev/null 2>&1 || fail "FreeBSD pkg is required"

LOG_FILE=$(mktemp "${TMPDIR:-/tmp}/freebsd-flatpak-install.log.XXXXXX") ||
    fail "could not create the installation log"
printf 'FreeBSD Flatpak installation log\nSource: %s\n' "$BASE" >"$LOG_FILE"

MISSING_PACKAGES=
for package_name in $REQUIRED_PACKAGES; do
    if ! pkg info -e "$package_name" >/dev/null 2>&1; then
        MISSING_PACKAGES="$MISSING_PACKAGES $package_name"
    fi
done

if [ -n "$MISSING_PACKAGES" ]; then
    ui_progress "Installing missing packages:${MISSING_PACKAGES}"
    # Package names are from the fixed list above; intentional word splitting.
    run_logged "Dependency installation" pkg install -y $MISSING_PACKAGES
fi

require_command() {
    command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

for command_name in awk cargo cat cc chown date env fetch gmake grep id install ldd mkdir \
    mkfifo mktemp patch pkg-config script sed sha256 sleep stty su tail tar tee tty; do
    require_command "$command_name"
done

BUILD_USER=${DOAS_USER:-${SUDO_USER:-}}
[ -n "$BUILD_USER" ] ||
    fail "could not determine the invoking user; run this installer from a normal user account with doas or sudo"
BUILD_UID=$(id -u "$BUILD_USER" 2>/dev/null) ||
    fail "invoking user does not exist: $BUILD_USER"
[ "$BUILD_UID" -ne 0 ] ||
    fail "the build must be owned by a normal user; run this installer with doas or sudo from that account"

[ -x "$LINUX_CC" ] || fail "Linux compiler not found or not executable: $LINUX_CC"
pkg-config --exists gio-2.0 gio-unix-2.0 glib-2.0 gdk-pixbuf-2.0 libpipewire-0.3 ||
    fail "missing development packages for GLib/GIO, GdkPixbuf, or PipeWire"

printf 'Build user: %s\n' "$BUILD_USER" >>"$LOG_FILE"
chown "$BUILD_USER" "$LOG_FILE" || fail "could not make the installation log readable by $BUILD_USER"

[ "$VERBOSE" -eq 1 ] || cursor_hide
cd "$BASE"
phase_start "Building private libostree 2026.3" "Configuring and building..." 0 0 0
run_logged "Private libostree build" su "$BUILD_USER" -c \
    "env INSTALL_PROGRESS_FD=3 INSTALL_SUPPRESS_PROGRESS=1 ./scripts/build-libostree.sh"
phase_finish

phase_start "Building FreeBSD Flatpak" "Compiling application..." 1 1
run_logged "Rust CLI build" su "$BUILD_USER" -c \
    "env PKG_CONFIG_PATH='$OSTREE_PREFIX/lib/pkgconfig' LIBRARY_PATH='$OSTREE_PREFIX/lib' cargo build --locked --release --bin flatpak"
phase_finish 0

phase_start "Building compatibility helpers" "Compiling native helpers..."
run_logged "Helper build-directory creation" su "$BUILD_USER" -c \
    "mkdir -p '$HELPER_BUILD_DIR'"
run_logged "Compatibility bridge build" su "$BUILD_USER" -c \
    "env INSTALL_PROGRESS_FD=3 INSTALL_SUPPRESS_PROGRESS=1 /bin/sh ./scripts/build-compatibility-bridges.sh '$HELPER_BUILD_DIR'"
run_logged "Wayland DRM compatibility shim build" su "$BUILD_USER" -c \
    "env PATH='$LINUX_BUILD_PATH' '$LINUX_CC' -shared -fPIC -O2 -Wall -Wextra compatibility_helpers/wayland-drm-devt-shim.c -o '$HELPER_BUILD_DIR/libwayland-drm-devt-shim.so' -ldl"
run_logged "DRM syncobj compatibility shim build" su "$BUILD_USER" -c \
    "env PATH='$LINUX_BUILD_PATH' '$LINUX_CC' -shared -fPIC -O2 -Wall -Wextra compatibility_helpers/drm-syncobj-errno-shim.c -o '$HELPER_BUILD_DIR/libdrm-syncobj-errno-shim.so' -ldl -pthread"
run_logged "Chromium zygote compatibility shim build" su "$BUILD_USER" -c \
    "env PATH='$LINUX_BUILD_PATH' '$LINUX_CC' -shared -fPIC -O2 -Wall -Wextra compatibility_helpers/chromium-zygote-drm-preload.c -o '$HELPER_BUILD_DIR/libchromium-zygote-drm-preload.so' -ldl -pthread"

for artifact in \
    target/release/flatpak \
    "$OSTREE_PREFIX/lib/libostree-1.so.1.0.0" \
    "$HELPER_BUILD_DIR/portal-bridge" \
    "$HELPER_BUILD_DIR/status-notifier-bridge" \
    "$HELPER_BUILD_DIR/libwayland-drm-devt-shim.so" \
    "$HELPER_BUILD_DIR/libdrm-syncobj-errno-shim.so" \
    "$HELPER_BUILD_DIR/libchromium-zygote-drm-preload.so"
do
    [ -s "$artifact" ] || fail "expected build artifact is missing or empty: $artifact"
done
phase_finish

phase_start "Installing FreeBSD Flatpak" "Installing files..."
run_logged "Installation directory creation" install -d -o root -g wheel -m 755 \
    "$INSTALL_BIN" "$INSTALL_LIBEXEC" "$INSTALL_LICENSES"
run_logged "CLI installation" install -o root -g wheel -m 755 \
    target/release/flatpak "$INSTALL_BIN/flatpak"
run_logged "Private libostree installation" install -o root -g wheel -m 755 \
    "$OSTREE_PREFIX/lib/libostree-1.so.1.0.0" \
    "$INSTALL_LIBEXEC/libostree-1.so.1"
run_logged "Project license installation" install -o root -g wheel -m 644 \
    LICENSE "$INSTALL_LICENSES/BSD-2-Clause.txt"
run_logged "Third-party license installation" install -o root -g wheel -m 644 \
    LICENSES/LGPL-2.0-or-later.txt LICENSES/MIT-ostree-rs.txt \
    "$INSTALL_LICENSES/"
run_logged "Compatibility helper installation" install -o root -g wheel -m 755 \
    "$HELPER_BUILD_DIR/portal-bridge" \
    "$HELPER_BUILD_DIR/status-notifier-bridge" \
    "$HELPER_BUILD_DIR/libwayland-drm-devt-shim.so" \
    "$HELPER_BUILD_DIR/libdrm-syncobj-errno-shim.so" \
    "$HELPER_BUILD_DIR/libchromium-zygote-drm-preload.so" \
    "$INSTALL_LIBEXEC/"

run_logged "Installed CLI dependency check" /bin/sh -c \
    "ldd_output=\$(ldd '$INSTALL_BIN/flatpak'); printf '%s\\n' \"\$ldd_output\"; if printf '%s\\n' \"\$ldd_output\" | grep -q 'not found'; then exit 1; fi"
phase_finish 0
print_install_tree

ui_heading "Installation complete"
ui_progress "Full log: $LOG_FILE"
