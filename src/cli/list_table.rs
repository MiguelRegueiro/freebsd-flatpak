use anyhow::{bail, Result};

const DEFAULT_COLUMNS: &[Column] = &[
    Column::Application,
    Column::Arch,
    Column::Branch,
    Column::Origin,
];
const ALL_COLUMNS: &[Column] = &[
    Column::Application,
    Column::Arch,
    Column::Branch,
    Column::Runtime,
    Column::Ref,
    Column::Origin,
    Column::Installation,
    Column::Active,
    Column::Size,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Column {
    Application,
    Arch,
    Branch,
    Runtime,
    Ref,
    Origin,
    Installation,
    Active,
    Size,
}

impl Column {
    fn name(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::Arch => "arch",
            Self::Branch => "branch",
            Self::Runtime => "runtime",
            Self::Ref => "ref",
            Self::Origin => "origin",
            Self::Installation => "installation",
            Self::Active => "active",
            Self::Size => "size",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Application => "Application ID",
            Self::Arch => "Arch",
            Self::Branch => "Branch",
            Self::Runtime => "Runtime",
            Self::Ref => "Ref",
            Self::Origin => "Origin",
            Self::Installation => "Installation",
            Self::Active => "Active commit",
            Self::Size => "Installed size",
        }
    }
}

pub(super) struct Options {
    pub(super) apps: bool,
    pub(super) runtimes: bool,
    columns: Vec<Column>,
    pub(super) columns_help: bool,
}

#[derive(Debug)]
pub(super) struct InstalledRow {
    pub(super) application: String,
    pub(super) arch: String,
    pub(super) branch: String,
    pub(super) runtime: String,
    pub(super) ref_name: String,
    pub(super) origin: String,
    pub(super) active: String,
    pub(super) installed_size: u64,
}

pub(super) fn parse_options(args: &[String]) -> Result<Options> {
    let mut app = false;
    let mut runtime = false;
    let mut show_details = false;
    let mut selectors = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "--app" => app = true,
            "--runtime" => runtime = true,
            "-d" | "--show-details" => show_details = true,
            "--columns" => {
                index += 1;
                selectors.push(
                    args.get(index)
                        .ok_or_else(|| anyhow::anyhow!("--columns requires a value"))?
                        .as_str(),
                );
            }
            _ if arg.starts_with("--columns=") => selectors.push(&arg[10..]),
            _ => bail!("unknown list option: {arg}"),
        }
        index += 1;
    }
    if !app && !runtime {
        app = true;
        runtime = true;
    }

    let mut columns_help = false;
    let columns = if show_details {
        ALL_COLUMNS.to_vec()
    } else if selectors.is_empty() {
        DEFAULT_COLUMNS.to_vec()
    } else {
        let mut columns = Vec::new();
        for selector in selectors {
            for field in selector.split(',') {
                let field = field.trim();
                if field.is_empty() {
                    bail!("empty list column");
                }
                match resolve_column(field)? {
                    Some(column) if !columns.contains(&column) => columns.push(column),
                    Some(_) => {}
                    None if field == "all" => {
                        for column in ALL_COLUMNS {
                            if !columns.contains(column) {
                                columns.push(*column);
                            }
                        }
                    }
                    None if field == "help" => columns_help = true,
                    None => unreachable!(),
                }
            }
        }
        columns
    };
    Ok(Options {
        apps: app,
        runtimes: runtime,
        columns,
        columns_help,
    })
}

fn resolve_column(field: &str) -> Result<Option<Column>> {
    if matches!(field, "all" | "help") {
        return Ok(None);
    }
    let matches = ALL_COLUMNS
        .iter()
        .copied()
        .filter(|column| column.name().starts_with(field))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [column] => Ok(Some(*column)),
        [] => bail!("unknown list column: {field}"),
        _ => bail!("ambiguous list column: {field}"),
    }
}

pub(super) fn render(rows: &[InstalledRow], options: &Options) -> String {
    if rows.is_empty() || options.columns.is_empty() {
        return String::new();
    }
    let values = rows
        .iter()
        .map(|row| {
            options
                .columns
                .iter()
                .map(|column| value(row, *column))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let widths = options
        .columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            values
                .iter()
                .map(|row| row[index].chars().count())
                .chain([column.title().chars().count()])
                .max()
                .unwrap_or(0)
        })
        .collect::<Vec<_>>();
    let mut output = String::new();
    write_table_line(
        &mut output,
        &options
            .columns
            .iter()
            .map(|column| column.title())
            .collect::<Vec<_>>(),
        &widths,
    );
    for row in &values {
        write_table_line(
            &mut output,
            &row.iter().map(String::as_str).collect::<Vec<_>>(),
            &widths,
        );
    }
    output
}

fn value(row: &InstalledRow, column: Column) -> String {
    match column {
        Column::Application => row.application.clone(),
        Column::Arch => row.arch.clone(),
        Column::Branch => row.branch.clone(),
        Column::Runtime => row.runtime.clone(),
        Column::Ref => row.ref_name.clone(),
        Column::Origin => row.origin.clone(),
        Column::Installation => "user".to_string(),
        Column::Active => row.active.clone(),
        Column::Size => super::size_format::format(row.installed_size),
    }
}

fn write_table_line(output: &mut String, values: &[&str], widths: &[usize]) {
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            output.push_str("  ");
        }
        output.push_str(value);
        if index + 1 < values.len() {
            output.extend(std::iter::repeat_n(
                ' ',
                widths[index].saturating_sub(value.chars().count()),
            ));
        }
    }
    output.push('\n');
}

pub(super) fn print_column_help() {
    println!("Available columns:");
    for column in ALL_COLUMNS {
        println!("  {:<12} {}", column.name(), column.title());
    }
    println!("  all          All supported columns");
    println!("  help         Show this column list");
}

#[cfg(test)]
#[path = "tests/list_table.rs"]
mod tests;
