use super::*;
use glib::variant::ToVariant;
use std::fs;
use std::process::{Command, Stdio};

#[test]
fn shared_portal_process_is_detached_from_the_creating_runner_session() {
    let mut command = Command::new("sleep");
    command.arg("30").stdin(Stdio::null());
    detach_shared_process(&mut command);
    let mut child = command.spawn().unwrap();
    let child_pid = child.id() as i32;

    assert_eq!(unsafe { libc::getsid(child_pid) }, child_pid);

    terminate_child(&mut child);
}

#[test]
fn two_connections_on_shared_bus_observe_one_name_owner() {
    let bus_dir = std::env::temp_dir().join(format!("ffp-dbus-{}", std::process::id()));
    let _ = fs::remove_dir_all(&bus_dir);
    fs::create_dir_all(&bus_dir).unwrap();
    let socket = bus_dir.join("bus");
    let config = bus_dir.join("session.conf");
    fs::write(&config, private_bus_config(&socket)).unwrap();
    let (mut child, address) = start_private_bus(&config).unwrap();
    let flags = gio::DBusConnectionFlags::AUTHENTICATION_CLIENT
        | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION;
    let first =
        gio::DBusConnection::for_address_sync(&address, flags, None, gio::Cancellable::NONE)
            .unwrap();
    let second =
        gio::DBusConnection::for_address_sync(&address, flags, None, gio::Cancellable::NONE)
            .unwrap();

    let requested = first
        .call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "RequestName",
            Some(&("org.example.App.Remote", 4u32).to_variant()),
            Some(glib::VariantTy::new("(u)").unwrap()),
            gio::DBusCallFlags::NONE,
            -1,
            gio::Cancellable::NONE,
        )
        .unwrap();
    assert_eq!(requested.get::<(u32,)>().unwrap().0, 1);

    let visible = second
        .call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            Some(&("org.example.App.Remote",).to_variant()),
            Some(glib::VariantTy::new("(b)").unwrap()),
            gio::DBusCallFlags::NONE,
            -1,
            gio::Cancellable::NONE,
        )
        .unwrap();
    assert!(visible.get::<(bool,)>().unwrap().0);

    drop(second);
    drop(first);
    terminate_child(&mut child);
    let _ = fs::remove_dir_all(&bus_dir);
}

#[test]
fn readiness_failure_only_names_missing_components() {
    let readiness = BridgeReadiness {
        file_chooser: false,
        screen_cast: false,
        status_notifier: true,
        document_portal: false,
    };

    assert_eq!(
        readiness.failure_message("/run/user/1001/doc"),
        "compatibility bridges did not publish FileChooser, ScreenCast, document mountpoint /run/user/1001/doc"
    );
}

#[test]
fn readiness_requires_every_component() {
    let mut readiness = BridgeReadiness {
        file_chooser: true,
        screen_cast: true,
        status_notifier: true,
        document_portal: true,
    };
    assert!(readiness.all_ready());

    readiness.status_notifier = false;
    assert!(!readiness.all_ready());
}
