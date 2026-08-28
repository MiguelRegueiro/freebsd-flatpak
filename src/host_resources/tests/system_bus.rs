use super::*;

#[test]
fn only_talk_system_bus_policies_are_exposed() {
    let metadata = "[System Bus Policy]\norg.example.Allowed=talk\norg.example.Denied=none\norg.example.Owner=own\n";
    assert_eq!(system_talk_names(metadata), vec!["org.example.Allowed"]);
}

#[test]
fn private_bus_scope_is_short_and_instance_specific() {
    assert_eq!(compact_scope("same"), compact_scope("same"));
    assert_ne!(compact_scope("first"), compact_scope("second"));
    assert_eq!(compact_scope("a-very-long-instance-identifier").len(), 16);
}
#[test]
fn private_bus_policy_is_destination_scoped_and_not_eavesdroppable() {
    let config = private_system_bus_config(
        Path::new("/tmp/private/system_bus_socket"),
        &[NETWORK_MANAGER_NAME.to_string()],
    );
    assert!(config.contains("<deny send_destination=\"*\"/>"));
    assert!(config.contains("<allow send_destination=\"org.freedesktop.NetworkManager\"/>"));
    assert!(config.contains("<deny eavesdrop=\"true\"/>"));
    assert!(!config.contains("/run/dbus/system_bus_socket"));
}

#[test]
fn no_system_bus_policy_does_not_create_a_mount() {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-system-bus-test-{}",
        std::process::id()
    ));
    let paths = Installation::for_test(&root);
    let mut bus =
        HostSystemBus::prepare(&paths, "[Context]\nshared=network;\n", "instance").unwrap();
    assert!(bus.runtime_mount().is_none());
    bus.cleanup().unwrap();
}
