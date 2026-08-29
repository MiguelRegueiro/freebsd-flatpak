use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn network_manager_connection_activation_lifecycle_over_private_bus() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = std::env::temp_dir().join(format!(
        "freebsd-flatpak-network-manager-compat-test-{}",
        std::process::id()
    ));
    let trace = std::env::temp_dir().join(format!(
        "freebsd-flatpak-network-manager-compat-test-{}-trace.log",
        std::process::id()
    ));
    let backend = std::env::temp_dir().join(format!(
        "freebsd-flatpak-network-manager-compat-test-{}-backend.sh",
        std::process::id()
    ));
    let pkg_config = Command::new("pkg-config")
        .args(["--cflags", "--libs", "gio-2.0", "gio-unix-2.0", "glib-2.0"])
        .output()
        .expect("run pkg-config for NetworkManager compatibility test");
    assert!(pkg_config.status.success(), "pkg-config failed");
    let flags = String::from_utf8(pkg_config.stdout).expect("pkg-config output is UTF-8");
    let compile_status = Command::new("cc")
        .current_dir(&root)
        .args(["-O2", "-Wall", "-Wextra", "-Werror"])
        .arg("compatibility_helpers/network_manager_compat.c")
        .arg("-o")
        .arg(&output)
        .args(flags.split_whitespace())
        .status()
        .expect("compile NetworkManager compatibility service");
    assert!(compile_status.success());
    std::fs::write(
        &backend,
        r#"#!/bin/sh
if [ "$1" = activate ]; then cat >/dev/null; echo 0123456789abcdef fwgtest 1 0; fi
"#,
    )
    .expect("write fake NetworkManager backend");
    let mut permissions = std::fs::metadata(&backend)
        .expect("stat fake backend")
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&backend, permissions).expect("make fake backend executable");

    let script = r#"
set -eu
helper=$1
trace=$2
backend=$3
"$helper" --address "$DBUS_SESSION_BUS_ADDRESS" --trace-file "$trace" --network-helper "$backend" &
helper_pid=$!
trap 'kill "$helper_pid" 2>/dev/null || true; wait "$helper_pid" 2>/dev/null || true' EXIT HUP INT TERM
attempt=0
while [ "$attempt" -lt 40 ]; do
    if gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus --method org.freedesktop.DBus.NameHasOwner org.freedesktop.NetworkManager | grep -q true; then break; fi
    attempt=$((attempt + 1)); sleep 0.05
done
[ "$attempt" -lt 40 ]
profile=$(gdbus call --session --dest org.freedesktop.NetworkManager --object-path /org/freedesktop/NetworkManager/Settings --method org.freedesktop.NetworkManager.Settings.AddConnectionUnsaved "{'connection': {'id': <'test'>, 'type': <'dummy'>}}" | sed -n "s/.*'\\([^']*\\)'.*/\\1/p")
[ "$profile" = /org/freedesktop/NetworkManager/Settings/1 ]
active=$(gdbus call --session --dest org.freedesktop.NetworkManager --object-path /org/freedesktop/NetworkManager --method org.freedesktop.NetworkManager.ActivateConnection "$profile" / / | sed -n "s/.*'\\([^']*\\)'.*/\\1/p")
[ "$active" = /org/freedesktop/NetworkManager/ActiveConnection/1 ]
gdbus call --session --dest org.freedesktop.NetworkManager --object-path /org/freedesktop/NetworkManager/Settings --method org.freedesktop.DBus.Properties.Get org.freedesktop.NetworkManager.Settings CanModify | grep -q '<true>'
gdbus call --session --dest org.freedesktop.NetworkManager --object-path "$profile" --method org.freedesktop.DBus.Properties.Get org.freedesktop.NetworkManager.Settings.Connection Flags | grep -q '<uint32 1>'
wireguard=$(gdbus call --session --dest org.freedesktop.NetworkManager --object-path /org/freedesktop/NetworkManager/Settings --method org.freedesktop.NetworkManager.Settings.AddConnectionUnsaved "{'connection': {'id': <'wireguard-test'>, 'type': <'wireguard'>}, 'wireguard': {'private-key': <'private-test'>, 'peers': <[{'public-key': <'public-test'>, 'endpoint': <'vpn.example.test:51820'>, 'allowed-ips': <['10.8.0.0/24']>, 'persistent-keepalive': <uint32 25>, 'preshared-key': <'peer-secret'>}]>}}" | sed -n "s/.*'\\([^']*\\)'.*/\\1/p")
wireguard_settings=$(gdbus call --session --dest org.freedesktop.NetworkManager --object-path "$wireguard" --method org.freedesktop.NetworkManager.Settings.Connection.GetSettings)
printf '%s\n' "$wireguard_settings" | grep -q public-test
! printf '%s\n' "$wireguard_settings" | grep -q -e private-key -e preshared-key -e private-test -e peer-secret
wireguard_secrets=$(gdbus call --session --dest org.freedesktop.NetworkManager --object-path "$wireguard" --method org.freedesktop.NetworkManager.Settings.Connection.GetSecrets wireguard)
printf '%s\n' "$wireguard_secrets" | grep -q -e private-test -e peer-secret
gdbus call --session --dest org.freedesktop.NetworkManager --object-path "$wireguard" --method org.freedesktop.NetworkManager.Settings.Connection.Delete
gdbus call --session --dest org.freedesktop.NetworkManager --object-path /org/freedesktop/NetworkManager --method org.freedesktop.DBus.Properties.Get org.freedesktop.NetworkManager ActiveConnections | grep -q "$active"
gdbus call --session --dest org.freedesktop.NetworkManager --object-path "$active" --method org.freedesktop.DBus.Properties.Get org.freedesktop.NetworkManager.Connection.Active State | grep -q "<uint32 2>"
gdbus call --session --dest org.freedesktop.NetworkManager --object-path "$active" --method org.freedesktop.DBus.Properties.Get org.freedesktop.NetworkManager.Connection.Active Default | grep -q "<true>"
gdbus call --session --dest org.freedesktop.NetworkManager --object-path "$active" --method org.freedesktop.DBus.Properties.Get org.freedesktop.NetworkManager.Connection.Active Default6 | grep -q "<false>"
gdbus call --session --dest org.freedesktop.NetworkManager --object-path /org/freedesktop/NetworkManager --method org.freedesktop.NetworkManager.DeactivateConnection "$active"
gdbus call --session --dest org.freedesktop.NetworkManager --object-path /org/freedesktop/NetworkManager --method org.freedesktop.DBus.Properties.Get org.freedesktop.NetworkManager ActiveConnections | grep -q '@ao \[\]'
gdbus call --session --dest org.freedesktop.NetworkManager --object-path "$profile" --method org.freedesktop.NetworkManager.Settings.Connection.Delete
gdbus call --session --dest org.freedesktop.NetworkManager --object-path /org/freedesktop --method org.freedesktop.DBus.ObjectManager.GetManagedObjects | grep -vq "$profile"
"#;
    let result = Command::new("dbus-run-session")
        .args(["--", "/bin/sh", "-c", script, "network-manager-test"])
        .arg(&output)
        .arg(&trace)
        .arg(&backend)
        .output()
        .expect("run NetworkManager compatibility service test");
    let trace_output = std::fs::read_to_string(&trace).expect("read NetworkManager trace");
    let _ = std::fs::remove_file(&output);
    let _ = std::fs::remove_file(&trace);
    let _ = std::fs::remove_file(&backend);
    assert!(
        result.status.success(),
        "NetworkManager compatibility test failed:\\nstdout:\\n{}\\nstderr:\\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(trace_output.contains("member=ActivateConnection"));
    assert!(trace_output.contains("member=DeactivateConnection"));
    assert!(trace_output.contains("member=InterfacesAdded"));
    assert!(trace_output.contains("member=InterfacesRemoved"));
    assert!(trace_output.contains("member=NewConnection"));
    assert!(trace_output.contains("member=ConnectionRemoved"));
    assert!(trace_output.contains("body=('org.freedesktop.NetworkManager.Settings', 'CanModify')"));
}

#[test]
fn network_manager_privileged_rolls_back_and_recovers_journaled_state() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let work = std::env::temp_dir().join(format!(
        "freebsd-flatpak-network-manager-privileged-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).expect("create privileged helper test directory");
    let mut work_permissions = std::fs::metadata(&work)
        .expect("stat privileged helper test directory")
        .permissions();
    work_permissions.set_mode(0o700);
    std::fs::set_permissions(&work, work_permissions)
        .expect("protect privileged helper test directory");
    let state_dir = work.join("state");
    let log = work.join("commands.log");
    let ifconfig = work.join("ifconfig");
    let route = work.join("route");
    let wg = work.join("wg");
    let resolvconf = work.join("resolvconf");
    for (path, body) in [
        (
            &ifconfig,
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$NM_TEST_LOG"
case "$1" in epair*) if [ "$2" = create ]; then echo "${1}a"; exit 0; fi ;; esac
if [ "$2" = up ] && [ "$NM_TEST_MODE" = failup ]; then exit 1; fi
if [ "$2" = destroy ] && [ "$NM_TEST_MODE" = cleanupfail ]; then exit 1; fi
exit 0
"#,
        ),
        (
            &route,
            r#"#!/bin/sh
printf 'route %s\n' "$*" >> "$NM_TEST_LOG"
if [ "$2" = get ]; then
  case "$3" in
    -inet) printf 'destination: %s\ngateway: 192.0.2.1\n' "$4" ;;
    -inet6) printf 'destination: %s\ngateway: 2001:db8::1\n' "$4" ;;
  esac
fi
exit 0
"#,
        ),
        (&wg, "#!/bin/sh\nexit 0\n"),
        (
            &resolvconf,
            r#"#!/bin/sh
printf 'resolvconf %s\n' "$*" >> "$NM_TEST_LOG"
if [ "$1" = -x ]; then cat >> "$NM_TEST_LOG"; fi
exit 0
"#,
        ),
    ] {
        std::fs::write(path, body).expect("write privileged helper command double");
        let mut permissions = std::fs::metadata(path)
            .expect("stat command double")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(path, permissions).expect("make command double executable");
    }
    let quote = |path: &std::path::Path| path.to_string_lossy().to_string();
    let harness = work.join("harness.c");
    let source = format!(
        r#"
#define STATE_PARENT "{}"
#define STATE_DIR "{}"
#define IFCONFIG "{}"
#define WG "{}"
#define ROUTE "{}"
#define RESOLVCONF "{}"
#define FREEBSD_FLATPAK_OWNER_UID 1001
#define NM_PRIVILEGED_TESTING 1
#define main network_manager_privileged_program_main
#include "{}"
#undef main
#include <stdio.h>
static int count_records(void) {{
  DIR *directory = opendir(STATE_DIR); if (!directory) return 0;
  int count = 0; struct dirent *entry; while ((entry = readdir(directory))) if (valid_token(entry->d_name)) count++;
  closedir(directory); return count;
}}
static void free_activation(Activation *activation) {{
  g_free(activation->token); g_free(activation->interface_name); g_free(activation->rename_interface_name);
  g_ptr_array_unref(activation->routes); g_ptr_array_unref(activation->endpoints);
}}
static void clear_log(void) {{ g_file_set_contents(g_getenv("NM_TEST_LOG"), "", 0, NULL); }}
static gboolean log_has(const char *text) {{ gchar *contents = NULL; gboolean found = g_file_get_contents(g_getenv("NM_TEST_LOG"), &contents, NULL, NULL) && strstr(contents, text); g_free(contents); return found; }}
int main(void) {{
  GError *error = NULL;
  g_setenv("NM_UNSAFE", "present", TRUE);
  if (!sanitize_environment(&error) || g_getenv("NM_UNSAFE") || !g_str_equal(g_getenv("PATH"), "/usr/sbin:/usr/bin:/sbin:/bin")) return 10;
  if (!ensure_state_layout(&error)) return 11;
  g_setenv("NM_TEST_LOG", "{}", TRUE); g_setenv("NM_TEST_MODE", "failup", TRUE);
  GVariantBuilder builder; g_variant_builder_init(&builder, G_VARIANT_TYPE("a{{sa{{sv}}}}"));
  GVariant *settings = g_variant_ref_sink(g_variant_builder_end(&builder));
  if (activate("dummy", settings, &error)) return 12;
  g_clear_error(&error); g_variant_unref(settings);
  if (count_records() != 0) return 13;
  GVariant *dns = g_variant_parse(G_VARIANT_TYPE("a{{sa{{sv}}}}"), "{{'ipv4': {{'dns-data': <[{{'address': <'192.0.2.53'>}}]>}}, 'ipv6': {{'dns-data': <[{{'address': <'2001:db8::53'>}}]>}}}}", NULL, NULL, &error);
  if (!dns) return 14;
  Activation ipv4 = {{ .token = g_strdup("11111111111111111111111111111111"), .interface_name = g_strdup("fwgfour"), .routes = g_ptr_array_new_with_free_func(g_free), .endpoints = g_ptr_array_new_with_free_func(g_free), .default_ipv4 = TRUE, .peer_routes = TRUE, .owner_uid = 1001, .owner_pid = 999999 }};
  g_ptr_array_add(ipv4.endpoints, g_strdup("198.51.100.20:51820"));
  if (!save_state(&ipv4, &error) || !configure_full_tunnel(&ipv4, &error) || !configure_dns(&ipv4, dns, &error)) return 15;
  if (!log_has("get -inet 198.51.100.20") || !log_has("-inet -net 0.0.0.0/1") || log_has("-inet6 -net ::/1") || !log_has("nameserver 192.0.2.53") || log_has("nameserver 2001:db8::53")) return 16;
  if (!cleanup_activation(&ipv4, TRUE, &error)) return 17;
  free_activation(&ipv4); clear_log();
  Activation ipv6 = {{ .token = g_strdup("22222222222222222222222222222222"), .interface_name = g_strdup("fwgsix"), .routes = g_ptr_array_new_with_free_func(g_free), .endpoints = g_ptr_array_new_with_free_func(g_free), .default_ipv6 = TRUE, .peer_routes = TRUE, .owner_uid = 1001, .owner_pid = 999999 }};
  g_ptr_array_add(ipv6.endpoints, g_strdup("[2001:db8::20]:51820"));
  if (!save_state(&ipv6, &error) || !configure_full_tunnel(&ipv6, &error) || !configure_dns(&ipv6, dns, &error)) return 18;
  if (!log_has("get -inet6 2001:db8::20") || !log_has("-inet6 -net ::/1") || log_has("-inet -net 0.0.0.0/1") || !log_has("nameserver 2001:db8::53") || log_has("nameserver 192.0.2.53")) return 19;
  if (!cleanup_activation(&ipv6, TRUE, &error)) return 20;
  free_activation(&ipv6); clear_log();
  Activation dual = {{ .token = g_strdup("33333333333333333333333333333333"), .interface_name = g_strdup("fwgdual"), .routes = g_ptr_array_new_with_free_func(g_free), .endpoints = g_ptr_array_new_with_free_func(g_free), .default_ipv4 = TRUE, .default_ipv6 = TRUE, .peer_routes = TRUE, .owner_uid = 1001, .owner_pid = 999999 }};
  if (!configure_dns(&dual, dns, &error) || !log_has("nameserver 192.0.2.53") || !log_has("nameserver 2001:db8::53")) return 21;
  if (!remove_dns(dual.interface_name, &error)) return 22;
  free_activation(&dual); clear_log();
  Activation failure = {{ .token = g_strdup("44444444444444444444444444444444"), .interface_name = g_strdup("fwgfail"), .rename_interface_name = g_strdup("wg42"), .routes = g_ptr_array_new_with_free_func(g_free), .endpoints = g_ptr_array_new_with_free_func(g_free), .owner_uid = 1001, .owner_pid = 999999, .dns_installed = TRUE }};
  g_ptr_array_add(failure.routes, g_strdup("4\t198.51.100.20\t192.0.2.1"));
  if (!save_state(&failure, &error)) return 23;
  g_setenv("NM_TEST_MODE", "cleanupfail", TRUE);
  if (cleanup_activation(&failure, TRUE, &error)) return 24;
  g_clear_error(&error);
  if (count_records() != 1 || !log_has("fwgfail destroy") || !log_has("wg42 destroy") || !log_has("delete -inet -host 198.51.100.20 192.0.2.1") || !log_has("resolvconf -f -d fwgfail.wireguard")) return 25;
  g_setenv("NM_TEST_MODE", "", TRUE);
  if (!cleanup_activation(&failure, TRUE, &error) || count_records() != 0) return 26;
  free_activation(&failure);
  Activation activation = {{ .token = g_strdup("0123456789abcdef0123456789abcdef"), .interface_name = g_strdup("fwgroll"), .routes = g_ptr_array_new_with_free_func(g_free), .endpoints = g_ptr_array_new_with_free_func(g_free), .owner_uid = 1001, .owner_pid = 999999 }};
  g_ptr_array_add(activation.routes, g_strdup("4\t198.51.100.20\t192.0.2.1"));
  if (!save_state(&activation, &error)) return 27;
  gchar *first = new_token(), *second = new_token();
  if (!valid_token(first) || !valid_token(second) || g_str_equal(first, second)) return 28;
  g_free(first); g_free(second); free_activation(&activation); g_variant_unref(dns);
  if (!recover_stale_activations(&error)) return 29;
  if (count_records() != 0) return 30;
  return 0;
}}
"#,
        quote(&work),
        quote(&state_dir),
        quote(&ifconfig),
        quote(&wg),
        quote(&route),
        quote(&resolvconf),
        quote(&root.join("compatibility_helpers/network_manager_privileged.c")),
        quote(&log)
    );
    std::fs::write(&harness, source).expect("write privileged helper harness");
    let pkg_config = Command::new("pkg-config")
        .args(["--cflags", "--libs", "gio-2.0", "gio-unix-2.0", "glib-2.0"])
        .output()
        .expect("run pkg-config for privileged helper test");
    assert!(pkg_config.status.success(), "pkg-config failed");
    let flags = String::from_utf8(pkg_config.stdout).expect("pkg-config output is UTF-8");
    let output = work.join("harness");
    let status = Command::new("cc")
        .args(["-O2", "-Wall", "-Wextra", "-Werror"])
        .arg(&harness)
        .arg("-o")
        .arg(&output)
        .args(flags.split_whitespace())
        .status()
        .expect("compile privileged helper harness");
    assert!(status.success());
    let result = Command::new(&output)
        .status()
        .expect("run privileged helper harness");
    let commands = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&work);
    assert!(
        result.success(),
        "privileged helper harness failed: {result}\ncommands:\n{commands}"
    );
    assert!(commands.contains("-i"));
}
