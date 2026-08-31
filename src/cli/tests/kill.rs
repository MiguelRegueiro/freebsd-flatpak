use super::*;
use anyhow::bail;
use std::cell::RefCell;

#[derive(Clone, Copy)]
enum Behavior {
    Signaled,
    Exited,
    Stale,
    Error,
}

struct FakeRequester {
    behavior: BTreeMap<libc::pid_t, Behavior>,
    requested: RefCell<Vec<libc::pid_t>>,
    terminated: RefCell<Vec<Vec<PathBuf>>>,
}

impl FakeRequester {
    fn new(behavior: impl IntoIterator<Item = (libc::pid_t, Behavior)>) -> Self {
        Self {
            behavior: behavior.into_iter().collect(),
            requested: RefCell::new(Vec::new()),
            terminated: RefCell::new(Vec::new()),
        }
    }
}

impl StopRequester for FakeRequester {
    fn request_launcher(&self, launcher: Launcher) -> Result<ForceStopResult> {
        self.requested.borrow_mut().push(launcher.pid);
        match self
            .behavior
            .get(&launcher.pid)
            .copied()
            .unwrap_or(Behavior::Signaled)
        {
            Behavior::Signaled => Ok(ForceStopResult::Signaled),
            Behavior::Exited => Ok(ForceStopResult::Exited),
            Behavior::Stale => Ok(ForceStopResult::Stale),
            Behavior::Error => bail!("permission denied"),
        }
    }

    fn terminate_roots(&self, roots: &[PathBuf]) -> Result<bool> {
        self.terminated.borrow_mut().push(roots.to_vec());
        Ok(true)
    }
}

fn record(app_id: &str, instance_id: &str, pid: libc::pid_t) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("app_id".to_string(), app_id.to_string()),
        ("instance_id".to_string(), instance_id.to_string()),
        ("launcher_pid".to_string(), pid.to_string()),
        ("launcher_start".to_string(), format!("{pid}:7")),
        ("root".to_string(), format!("/sandboxes/{instance_id}")),
    ])
}

fn no_processes() -> sandbox::SandboxProcessSnapshot {
    sandbox::SandboxProcessSnapshot::for_test(Vec::new())
}

#[test]
fn parsing_requires_exactly_one_target() {
    assert_eq!(
        parse_args(vec!["org.example.App".to_string()]).unwrap(),
        "org.example.App"
    );
    assert_eq!(
        parse_args(Vec::new()).unwrap_err().to_string(),
        "usage: flatpak kill INSTANCE"
    );
    assert_eq!(
        parse_args(vec!["one".to_string(), "two".to_string()])
            .unwrap_err()
            .to_string(),
        "usage: flatpak kill INSTANCE"
    );
}

#[test]
fn instance_id_stops_only_the_selected_instance() {
    let requester = FakeRequester::new([]);
    kill_records(
        vec![
            record("org.example.App", "instance-one", 101),
            record("org.example.App", "instance-two", 102),
            record("org.example.Other", "instance-three", 103),
        ],
        "instance-two",
        &no_processes(),
        &requester,
    )
    .unwrap();

    assert_eq!(*requester.requested.borrow(), vec![102]);
}

#[test]
fn application_id_stops_every_instance_of_the_application() {
    let requester = FakeRequester::new([]);
    kill_records(
        vec![
            record("org.example.App", "instance-one", 101),
            record("org.example.Other", "instance-other", 102),
            record("org.example.App", "instance-two", 103),
        ],
        "org.example.App",
        &no_processes(),
        &requester,
    )
    .unwrap();

    assert_eq!(*requester.requested.borrow(), vec![101, 103]);
}

#[test]
fn absent_and_stale_instances_report_not_running() {
    let absent = FakeRequester::new([]);
    assert_eq!(
        kill_records(
            vec![record("org.example.Other", "other", 101)],
            "org.example.App",
            &no_processes(),
            &absent,
        )
        .unwrap_err()
        .to_string(),
        "org.example.App is not running"
    );

    let stale = FakeRequester::new([(101, Behavior::Stale)]);
    assert_eq!(
        kill_records(
            vec![record("org.example.App", "stale", 101)],
            "org.example.App",
            &no_processes(),
            &stale,
        )
        .unwrap_err()
        .to_string(),
        "org.example.App is not running"
    );

    let mut malformed = record("org.example.App", "malformed", 102);
    malformed.remove("launcher_start");
    assert_eq!(
        kill_records(vec![malformed], "org.example.App", &no_processes(), &absent,)
            .unwrap_err()
            .to_string(),
        "org.example.App is not running"
    );
}

#[test]
fn exit_racing_with_force_stop_is_success() {
    let requester = FakeRequester::new([(101, Behavior::Exited)]);
    kill_records(
        vec![record("org.example.App", "racing", 101)],
        "racing",
        &no_processes(),
        &requester,
    )
    .unwrap();
}

#[test]
fn errors_are_reported_after_all_selected_instances_are_attempted() {
    let requester = FakeRequester::new([(101, Behavior::Error), (102, Behavior::Signaled)]);
    let error = kill_records(
        vec![
            record("org.example.App", "first", 101),
            record("org.example.App", "second", 102),
        ],
        "org.example.App",
        &no_processes(),
        &requester,
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("failed to stop org.example.App"));
    assert!(error.contains("instance first (launcher 101): permission denied"));
    assert_eq!(*requester.requested.borrow(), vec![101, 102]);
}

#[test]
fn dead_launcher_with_owned_descendants_uses_scoped_termination() {
    let requester = FakeRequester::new([(101, Behavior::Stale)]);
    let processes = sandbox::SandboxProcessSnapshot::for_test(vec![
        (501, PathBuf::from("/sandboxes/launcherless/work")),
        (502, PathBuf::from("/sandboxes/launcherless")),
    ]);

    kill_records(
        vec![record("org.example.App", "launcherless", 101)],
        "org.example.App",
        &processes,
        &requester,
    )
    .unwrap();

    assert_eq!(*requester.requested.borrow(), vec![101]);
    assert_eq!(
        *requester.terminated.borrow(),
        vec![vec![PathBuf::from("/sandboxes/launcherless")]]
    );
}

#[test]
fn unrelated_process_reference_is_never_selected_for_termination() {
    let requester = FakeRequester::new([(101, Behavior::Stale)]);
    let processes = sandbox::SandboxProcessSnapshot::for_test(vec![(
        501,
        PathBuf::from("/unrelated/host-process"),
    )]);

    let error = kill_records(
        vec![record("org.example.App", "stale", 101)],
        "org.example.App",
        &processes,
        &requester,
    )
    .unwrap_err();

    assert_eq!(error.to_string(), "org.example.App is not running");
    assert!(requester.terminated.borrow().is_empty());
}

#[test]
fn nested_descendant_is_owned_by_its_parent_instance() {
    let requester = FakeRequester::new([(101, Behavior::Stale)]);
    let parent = record("org.example.App", "parent", 101);
    let parent_root = PathBuf::from(parent.get("root").unwrap());
    let nested_root = PathBuf::from("/sandboxes/parent-nested-1");
    let nested = BTreeMap::from([
        ("app_id".to_string(), "org.example.App".to_string()),
        ("instance_id".to_string(), "parent-nested-1".to_string()),
        ("root".to_string(), nested_root.display().to_string()),
        ("parent_root".to_string(), parent_root.display().to_string()),
    ]);
    let processes =
        sandbox::SandboxProcessSnapshot::for_test(vec![(601, nested_root.join("work"))]);

    kill_records(
        vec![parent, nested],
        "org.example.App",
        &processes,
        &requester,
    )
    .unwrap();

    assert_eq!(
        *requester.terminated.borrow(),
        vec![vec![parent_root, nested_root]]
    );
}

#[test]
fn system_stop_terminates_only_processes_bound_to_the_sandbox() {
    let base =
        std::env::temp_dir().join(format!("freebsd-flatpak-kill-owned-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("sandbox");
    let outside = base.join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let mut first = std::process::Command::new("sleep")
        .arg("30")
        .current_dir(&root)
        .spawn()
        .unwrap();
    let mut second = std::process::Command::new("sleep")
        .arg("30")
        .current_dir(root.join("."))
        .spawn()
        .unwrap();
    let mut unrelated = std::process::Command::new("sleep")
        .arg("30")
        .current_dir(&outside)
        .spawn()
        .unwrap();
    let mut owned_record = record("org.example.App", "owned", i32::MAX);
    owned_record.insert("root".to_string(), root.display().to_string());
    let processes = sandbox::SandboxProcessSnapshot::capture().unwrap();

    kill_records(
        vec![owned_record],
        "org.example.App",
        &processes,
        &SystemStopRequester,
    )
    .unwrap();

    first.wait().unwrap();
    second.wait().unwrap();
    assert!(unrelated.try_wait().unwrap().is_none());
    unrelated.kill().unwrap();
    unrelated.wait().unwrap();
    std::fs::remove_dir_all(base).unwrap();
}
