use crate::installation as state;
use crate::installation::installation_paths::Installation;
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
        if launcher_pid <= 0 || !state::run_record_launcher_active(&record)? {
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

pub(crate) fn cmd_ps(paths: &Installation, args: Vec<String>) -> Result<()> {
    print!("{}", output(paths, args)?);
    Ok(())
}

#[cfg(test)]
#[path = "tests/ps.rs"]
mod tests;
