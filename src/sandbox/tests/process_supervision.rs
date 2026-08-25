use super::*;
use anyhow::anyhow;
use std::cell::Cell;

#[test]
fn process_group_parser_selects_only_matching_processes() {
    let processes = "  PID  PGID\n  100   100\n  101   100\n  200   200\n";

    assert_eq!(
        process_group_pids(processes, 100).collect::<Vec<_>>(),
        vec![100, 101]
    );
}

#[test]
fn supervision_waits_until_the_process_group_is_empty() {
    let checks = Cell::new(0);
    let pauses = Cell::new(0);

    wait_while_processes_remain(
        || {
            let check = checks.get();
            checks.set(check + 1);
            Ok(check < 2)
        },
        || false,
        || pauses.set(pauses.get() + 1),
    )
    .unwrap();

    assert_eq!(checks.get(), 3);
    assert_eq!(pauses.get(), 2);
}

#[test]
fn supervision_propagates_process_inspection_failures() {
    let pauses = Cell::new(0);

    let error = wait_while_processes_remain(
        || Err(anyhow!("process inspection failed")),
        || false,
        || pauses.set(pauses.get() + 1),
    )
    .unwrap_err();

    assert_eq!(error.to_string(), "process inspection failed");
    assert_eq!(pauses.get(), 0);
}

#[test]
fn supervision_stops_waiting_when_termination_is_requested() {
    let inspections = Cell::new(0);
    let pauses = Cell::new(0);

    wait_while_processes_remain(
        || {
            inspections.set(inspections.get() + 1);
            Ok(true)
        },
        || true,
        || pauses.set(pauses.get() + 1),
    )
    .unwrap();

    assert_eq!(inspections.get(), 0);
    assert_eq!(pauses.get(), 0);
}
