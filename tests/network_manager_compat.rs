use std::path::PathBuf;
use std::process::Command;

#[test]
fn read_only_network_manager_contract_over_private_bus() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = std::env::temp_dir().join(format!(
        "freebsd-flatpak-network-manager-compat-test-{}",
        std::process::id()
    ));
    let trace = std::env::temp_dir().join(format!(
        "freebsd-flatpak-network-manager-compat-test-{}-trace.log",
        std::process::id()
    ));
    let pkg_config = Command::new("pkg-config")
        .args(["--cflags", "--libs", "gio-2.0", "gio-unix-2.0", "glib-2.0"])
        .output()
        .expect("run pkg-config for NetworkManager compatibility test");
    assert!(pkg_config.status.success(), "pkg-config failed");
    let flags = String::from_utf8(pkg_config.stdout).expect("pkg-config output is UTF-8");

    let mut compiler = Command::new("cc");
    compiler
        .current_dir(&root)
        .args(["-O2", "-Wall", "-Wextra", "-Werror"])
        .arg("compatibility_helpers/network_manager_compat.c")
        .arg("-o")
        .arg(&output)
        .args(flags.split_whitespace());
    let compile_status = compiler
        .status()
        .expect("compile NetworkManager compatibility service");
    assert!(
        compile_status.success(),
        "NetworkManager compatibility service did not compile"
    );

    let script = r#"
set -e
helper=$1
trace=$2
"$helper" --address "$DBUS_SESSION_BUS_ADDRESS" --trace-file "$trace" &
helper_pid=$!
trap 'kill "$helper_pid" 2>/dev/null || true; wait "$helper_pid" 2>/dev/null || true' EXIT HUP INT TERM

ready=false
attempt=0
while [ "$attempt" -lt 40 ]; do
    if gdbus call --session --dest org.freedesktop.DBus \
        --object-path /org/freedesktop/DBus \
        --method org.freedesktop.DBus.NameHasOwner \
        org.freedesktop.NetworkManager | grep -q true; then
        ready=true
        break
    fi
    attempt=$((attempt + 1))
    sleep 0.05
done
[ "$ready" = true ]

gdbus call --session --dest org.freedesktop.NetworkManager \
    --object-path /org/freedesktop \
    --method org.freedesktop.DBus.ObjectManager.GetManagedObjects

gdbus call --session --dest org.freedesktop.NetworkManager \
    --object-path /org/freedesktop/NetworkManager \
    --method org.freedesktop.DBus.Properties.Get \
    org.freedesktop.NetworkManager Version

gdbus call --session --dest org.freedesktop.NetworkManager \
    --object-path /org/freedesktop/NetworkManager/Settings \
    --method org.freedesktop.NetworkManager.Settings.ListConnections

gdbus call --session --dest org.freedesktop.NetworkManager \
    --object-path /org/freedesktop/NetworkManager/Settings \
    --method org.freedesktop.NetworkManager.Settings.AddConnectionUnsaved \
    "{'connection': {'id': <'test'>, 'type': <'dummy'>}}"
gdbus call --session --dest org.freedesktop.NetworkManager \
    --object-path /org/freedesktop \
    --method org.freedesktop.DBus.ObjectManager.GetManagedObjects | grep -q /org/freedesktop/NetworkManager/Settings/1
gdbus call --session --dest org.freedesktop.NetworkManager \
    --object-path /org/freedesktop/NetworkManager/Settings \
    --method org.freedesktop.DBus.Properties.Get \
    org.freedesktop.NetworkManager.Settings Connections | grep -q /org/freedesktop/NetworkManager/Settings/1
gdbus call --session --dest org.freedesktop.NetworkManager \
    --object-path /org/freedesktop/NetworkManager/Settings/1 \
    --method org.freedesktop.NetworkManager.Settings.Connection.GetSettings | grep -q test

gdbus call --session --dest org.freedesktop.NetworkManager \
    --object-path /org/freedesktop/NetworkManager/Settings/1 \
    --method org.freedesktop.NetworkManager.Settings.Connection.Delete
gdbus call --session --dest org.freedesktop.NetworkManager \
    --object-path /org/freedesktop \
    --method org.freedesktop.DBus.ObjectManager.GetManagedObjects | grep -vq /org/freedesktop/NetworkManager/Settings/1
if mutation_error=$(gdbus call --session \
    --dest org.freedesktop.NetworkManager \
    --object-path /org/freedesktop/NetworkManager \
    --method org.freedesktop.NetworkManager.Enable true 2>&1); then
    echo "mutation unexpectedly succeeded" >&2
    exit 1
fi
echo "$mutation_error" | grep -q org.freedesktop.NetworkManager.Error.NotSupported
"#;

    let result = Command::new("dbus-run-session")
        .args(["--", "/bin/sh", "-c", script, "network-manager-test"])
        .arg(&output)
        .arg(&trace)
        .output()
        .expect("run NetworkManager compatibility service test");
    let trace_output = std::fs::read_to_string(&trace).expect("read NetworkManager trace");
    let _ = std::fs::remove_file(&output);
    let _ = std::fs::remove_file(&trace);

    assert!(
        result.status.success(),
        "NetworkManager compatibility test failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stdout.contains("1.40.18-freebsd-compat"));
    assert!(stdout.contains("@ao []"));
    assert!(stdout.contains("/org/freedesktop/NetworkManager"));
    assert!(stdout.contains("/org/freedesktop/NetworkManager/Settings/1"));
    assert!(trace_output.contains("member=InterfacesAdded"));
    assert!(trace_output.contains("member=InterfacesRemoved"));
    for trace in [&stderr[..], &trace_output] {
        assert!(trace.contains("interface=org.freedesktop.DBus.ObjectManager"));
        assert!(trace.contains("member=GetManagedObjects"));
        assert!(trace.contains("interface=org.freedesktop.DBus.Properties"));
        assert!(trace.contains("member=Get"));
        assert!(trace.contains("interface=org.freedesktop.NetworkManager.Settings"));
        assert!(trace.contains("member=ListConnections"));
        assert!(trace.contains("member=AddConnectionUnsaved"));
    }
}
