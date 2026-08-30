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
}

impl FakeRequester {
    fn new(behavior: impl IntoIterator<Item = (libc::pid_t, Behavior)>) -> Self {
        Self {
            behavior: behavior.into_iter().collect(),
            requested: RefCell::new(Vec::new()),
        }
    }
}

impl StopRequester for FakeRequester {
    fn request(&self, instance: &RunningInstance) -> Result<ForceStopResult> {
        self.requested.borrow_mut().push(instance.launcher_pid);
        match self
            .behavior
            .get(&instance.launcher_pid)
            .copied()
            .unwrap_or(Behavior::Signaled)
        {
            Behavior::Signaled => Ok(ForceStopResult::Signaled),
            Behavior::Exited => Ok(ForceStopResult::Exited),
            Behavior::Stale => Ok(ForceStopResult::Stale),
            Behavior::Error => bail!("permission denied"),
        }
    }
}

fn record(app_id: &str, instance_id: &str, pid: libc::pid_t) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("app_id".to_string(), app_id.to_string()),
        ("instance_id".to_string(), instance_id.to_string()),
        ("launcher_pid".to_string(), pid.to_string()),
        ("launcher_start".to_string(), format!("{pid}:7")),
    ])
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
            &stale,
        )
        .unwrap_err()
        .to_string(),
        "org.example.App is not running"
    );

    let mut malformed = record("org.example.App", "malformed", 102);
    malformed.remove("launcher_start");
    assert_eq!(
        kill_records(vec![malformed], "org.example.App", &absent)
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
        &requester,
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("failed to stop org.example.App"));
    assert!(error.contains("instance first (launcher 101): permission denied"));
    assert_eq!(*requester.requested.borrow(), vec![101, 102]);
}
