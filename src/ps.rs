use crate::paths::Installation;
use crate::state;
use anyhow::{bail, Context, Result};

const DEFAULT_COLUMNS: &[Column] = &[
    Column::Instance,
    Column::Pid,
    Column::Application,
    Column::Runtime,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Column {
    Instance,
    Pid,
    ChildPid,
    Application,
    Arch,
    Branch,
    Commit,
    Runtime,
    RuntimeBranch,
    RuntimeCommit,
}

impl Column {
    const ALL: [Self; 10] = [
        Self::Instance,
        Self::Pid,
        Self::ChildPid,
        Self::Application,
        Self::Arch,
        Self::Branch,
        Self::Commit,
        Self::Runtime,
        Self::RuntimeBranch,
        Self::RuntimeCommit,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Instance => "instance",
            Self::Pid => "pid",
            Self::ChildPid => "child-pid",
            Self::Application => "application",
            Self::Arch => "arch",
            Self::Branch => "branch",
            Self::Commit => "commit",
            Self::Runtime => "runtime",
            Self::RuntimeBranch => "runtime-branch",
            Self::RuntimeCommit => "runtime-commit",
        }
    }

    fn heading(self) -> &'static str {
        match self {
            Self::Instance => "Instance",
            Self::Pid => "PID",
            Self::ChildPid => "Child-PID",
            Self::Application => "Application",
            Self::Arch => "Arch",
            Self::Branch => "Branch",
            Self::Commit => "Commit",
            Self::Runtime => "Runtime",
            Self::RuntimeBranch => "R.-Branch",
            Self::RuntimeCommit => "R.-Commit",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        if let Some(column) = Self::ALL.into_iter().find(|column| column.name() == value) {
            return Ok(column);
        }
        let matches = Self::ALL
            .into_iter()
            .filter(|column| column.name().starts_with(value))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [column] => Ok(*column),
            [] => bail!("unknown ps column: {value}"),
            _ => bail!("ambiguous ps column: {value}"),
        }
    }
}

struct Instance {
    id: String,
    launcher_pid: String,
    child_pid: String,
    app: state::AppRecord,
}

pub fn output(paths: &Installation, args: Vec<String>) -> Result<String> {
    let columns = parse_columns(args)?;
    let mut instances = Vec::new();
    for record in state::read_run_records(paths)? {
        let launcher_pid = record
            .get("launcher_pid")
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0);
        if launcher_pid <= 0 || !process_alive(launcher_pid) {
            continue;
        }
        instances.push(Instance {
            id: record
                .get("instance_id")
                .cloned()
                .unwrap_or_else(|| launcher_pid.to_string()),
            launcher_pid: launcher_pid.to_string(),
            child_pid: record
                .get("child_pid")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(0)
                .to_string(),
            app: state::app_from_run_record(paths, &record)?,
        });
    }
    instances.sort_by(|left, right| left.id.cmp(&right.id));
    if instances.is_empty() {
        return Ok(String::new());
    }

    let rows = instances
        .iter()
        .map(|instance| {
            columns
                .iter()
                .map(|column| value(instance, *column))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    Ok(render_table(&columns, &rows))
}

fn parse_columns(args: Vec<String>) -> Result<Vec<Column>> {
    if args.is_empty() {
        return Ok(DEFAULT_COLUMNS.to_vec());
    }
    let mut values = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let value = if arg == "--columns" {
            args.next().context("missing value for --columns")?
        } else if let Some(value) = arg.strip_prefix("--columns=") {
            value.to_string()
        } else {
            bail!("usage: flatpak ps [--columns=FIELD,...]");
        };
        values.extend(value.split(',').map(str::to_string));
    }
    if values.iter().any(String::is_empty) {
        bail!("ps columns must not be empty");
    }
    values.iter().map(|value| Column::parse(value)).collect()
}

fn value(instance: &Instance, column: Column) -> String {
    let runtime = instance.app.runtime_ref.split('/').collect::<Vec<_>>();
    match column {
        Column::Instance => instance.id.clone(),
        Column::Pid => instance.launcher_pid.clone(),
        Column::ChildPid => instance.child_pid.clone(),
        Column::Application => instance.app.app_id.clone(),
        Column::Arch => instance.app.arch.clone(),
        Column::Branch => instance.app.branch.clone(),
        Column::Commit => instance.app.app_commit.clone(),
        Column::Runtime => runtime.first().unwrap_or(&"").to_string(),
        Column::RuntimeBranch => runtime.get(2).unwrap_or(&"").to_string(),
        Column::RuntimeCommit => instance.app.runtime_commit.clone(),
    }
}

fn render_table(columns: &[Column], rows: &[Vec<String>]) -> String {
    let widths = columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            rows.iter()
                .map(|row| row[index].len())
                .max()
                .unwrap_or(0)
                .max(column.heading().len())
        })
        .collect::<Vec<_>>();
    let mut output = String::new();
    let headings = columns
        .iter()
        .map(|column| column.heading().to_string())
        .collect::<Vec<_>>();
    append_row(&mut output, &headings, &widths);
    for row in rows {
        append_row(&mut output, row, &widths);
    }
    output
}

fn append_row(output: &mut String, row: &[String], widths: &[usize]) {
    for (index, value) in row.iter().enumerate() {
        output.push_str(value);
        if index + 1 < row.len() {
            output.push_str(&" ".repeat(widths[index] - value.len() + 1));
        }
    }
    output.push('\n');
}

fn process_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_paths(name: &str) -> Installation {
        let root =
            std::env::temp_dir().join(format!("freebsd-flatpak-ps-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        Installation::for_test(&root)
    }

    fn write_app(paths: &Installation) {
        state::ensure_layout(paths).unwrap();
        fs::create_dir_all(paths.refs().join("apps")).unwrap();
        fs::write(
            paths.refs().join("apps/app.zen_browser.zen.ini"),
            "app_id=app.zen_browser.zen\napp_ref=app/app.zen_browser.zen/x86_64/stable\napp_commit=app-commit\napp_dir=apps/app.zen_browser.zen\narch=x86_64\nbranch=stable\nruntime_ref=org.freedesktop.Platform/x86_64/25.08\nruntime_commit=runtime-commit\nruntime_dir=runtimes/org.freedesktop.Platform-25.08\ncommand=zen\n",
        )
        .unwrap();
    }

    #[test]
    fn default_and_selected_columns_match_flatpak_ps() {
        let paths = test_paths("columns");
        write_app(&paths);
        state::write_run_record(
            &paths,
            "app.zen_browser.zen",
            "815848674",
            &paths.chroots().join("zen/815848674"),
            std::process::id(),
            5960,
        )
        .unwrap();

        let default = output(&paths, Vec::new()).unwrap();
        assert!(default.starts_with("Instance  PID"));
        assert!(default.contains("815848674"));
        assert!(default.contains("app.zen_browser.zen"));
        assert!(default.contains("org.freedesktop.Platform"));

        let selected = output(
            &paths,
            vec!["--columns=instance,application,pid,child-pid".to_string()],
        )
        .unwrap();
        assert!(selected.starts_with("Instance  Application"));
        assert!(selected.contains(&format!("{} 5960", std::process::id())));
    }

    #[test]
    fn stale_records_are_not_shown() {
        let paths = test_paths("stale");
        write_app(&paths);
        state::write_run_record(
            &paths,
            "app.zen_browser.zen",
            "stale",
            &paths.chroots().join("zen/stale"),
            i32::MAX as u32,
            0,
        )
        .unwrap();

        assert_eq!(output(&paths, Vec::new()).unwrap(), "");
    }

    #[test]
    fn pinned_columns_do_not_follow_a_later_current_generation() {
        let paths = test_paths("pinned-generation");
        write_app(&paths);
        let old = state::get_app(&paths, "app.zen_browser.zen").unwrap();
        state::write_pinned_run_record(
            &paths,
            "pinned",
            &paths.chroots().join("pinned"),
            std::process::id(),
            0,
            &old,
        )
        .unwrap();
        fs::write(
            paths.refs().join("apps/app.zen_browser.zen.ini"),
            "app_id=app.zen_browser.zen\napp_ref=app/app.zen_browser.zen/x86_64/stable\napp_commit=app-new\napp_dir=apps/app.zen_browser.zen/app-new\narch=x86_64\nbranch=stable\nruntime_ref=org.freedesktop.Platform/x86_64/25.08\nruntime_commit=runtime-new\nruntime_dir=runtimes/org.freedesktop.Platform-25.08/runtime-new\ncommand=zen\n",
        )
        .unwrap();

        let output = output(&paths, vec!["--columns=commit,runtime-commit".to_string()]).unwrap();
        assert!(output.contains("app-commit"));
        assert!(output.contains("runtime-commit"));
        assert!(!output.contains("app-new"));
        assert!(!output.contains("runtime-new"));
    }

    #[test]
    fn active_legacy_records_use_the_launcher_pid_as_instance() {
        let paths = test_paths("legacy");
        write_app(&paths);
        fs::write(
            paths.runs().join("app.zen_browser.zen.ini"),
            format!(
                "app_id=app.zen_browser.zen\nroot=/legacy\nlauncher_pid={}\nchild_pid=5960\n",
                std::process::id()
            ),
        )
        .unwrap();

        let result = output(
            &paths,
            vec!["--columns=instance,application,pid,child-pid".to_string()],
        )
        .unwrap();
        let pid = std::process::id().to_string();
        assert!(result.lines().nth(1).unwrap().starts_with(&pid));
        assert!(result.contains(&format!("{pid} 5960")));
    }

    #[test]
    fn columns_accept_unique_prefixes_and_reject_unknown_names() {
        assert_eq!(
            parse_columns(vec!["--columns=inst,app,child,runtime".to_string()]).unwrap(),
            vec![
                Column::Instance,
                Column::Application,
                Column::ChildPid,
                Column::Runtime
            ]
        );
        assert!(parse_columns(vec!["--columns=nope".to_string()]).is_err());
    }
}
