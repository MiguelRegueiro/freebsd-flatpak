use crate::flatpak_ref::{PartialRef, RefKind};
use anyhow::{bail, Context, Result};

const COLUMN_SPACING: usize = 4;

const DEFAULT_COLUMNS: &[Column] = &[
    Column::Name,
    Column::Application,
    Column::Version,
    Column::Branch,
    Column::Origin,
];
const ALL_COLUMNS: &[Column] = &[
    Column::Name,
    Column::Application,
    Column::Arch,
    Column::Version,
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
    Name,
    Application,
    Arch,
    Version,
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
            Self::Name => "name",
            Self::Application => "application",
            Self::Arch => "arch",
            Self::Version => "version",
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
            Self::Name => "Name",
            Self::Application => "Application ID",
            Self::Arch => "Arch",
            Self::Version => "Version",
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
    pub(super) all: bool,
    pub(super) app_runtime: Option<PartialRef>,
    columns: Vec<Column>,
    pub(super) columns_help: bool,
}

#[derive(Debug)]
pub(super) struct InstalledRow {
    pub(super) name: String,
    pub(super) application: String,
    pub(super) arch: String,
    pub(super) version: String,
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
    let mut all = false;
    let mut app_runtime = None;
    let mut show_details = false;
    let mut selectors = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "--app" => app = true,
            "--runtime" => runtime = true,
            "-a" | "--all" => all = true,
            "--app-runtime" => {
                index += 1;
                let value = args.get(index).context("missing value for --app-runtime")?;
                set_app_runtime(&mut app_runtime, value)?;
            }
            _ if arg.starts_with("--app-runtime=") => {
                set_app_runtime(&mut app_runtime, &arg[14..])?;
            }
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
    if app_runtime.is_some() {
        if runtime {
            bail!("--app-runtime cannot be used with --runtime");
        }
        app = true;
    } else if !app && !runtime {
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
        all,
        app_runtime,
        columns,
        columns_help,
    })
}

fn set_app_runtime(target: &mut Option<PartialRef>, value: &str) -> Result<()> {
    if target.is_some() {
        bail!("--app-runtime may only be specified once");
    }
    let partial = PartialRef::parse(value)?;
    partial.effective_kind(Some(RefKind::Runtime))?;
    *target = Some(partial);
    Ok(())
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

pub(super) fn terminal_width() -> Option<usize> {
    let mut size = std::mem::MaybeUninit::<libc::winsize>::zeroed();
    // SAFETY: `size` points to writable storage for the `winsize` requested by
    // TIOCGWINSZ, and stdout remains open for the duration of the call.
    let result = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, size.as_mut_ptr()) };
    if result != 0 {
        return None;
    }
    // SAFETY: a successful TIOCGWINSZ call initialized the winsize structure.
    let columns = unsafe { size.assume_init() }.ws_col as usize;
    (columns > 0).then_some(columns)
}

pub(super) fn render(
    rows: &[InstalledRow],
    options: &Options,
    styled: bool,
    terminal_width: Option<usize>,
) -> String {
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
    let mut widths = options
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
    let spacing = fit_widths(&mut widths, terminal_width);
    let mut output = String::new();
    write_table_line(
        &mut output,
        &options
            .columns
            .iter()
            .map(|column| column.title())
            .collect::<Vec<_>>(),
        &widths,
        spacing,
        styled,
    );
    for row in &values {
        write_table_line(
            &mut output,
            &row.iter().map(String::as_str).collect::<Vec<_>>(),
            &widths,
            spacing,
            false,
        );
    }
    output
}

fn fit_widths(widths: &mut [usize], terminal_width: Option<usize>) -> usize {
    let Some(terminal_width) = terminal_width else {
        return COLUMN_SPACING;
    };
    let separators = widths.len().saturating_sub(1);
    let spacing = COLUMN_SPACING.min(
        terminal_width
            .saturating_sub(widths.len())
            .checked_div(separators)
            .unwrap_or(0),
    );
    let available = terminal_width.saturating_sub(spacing * separators);
    while widths.iter().sum::<usize>() > available {
        let Some((index, _)) = widths
            .iter()
            .enumerate()
            .filter(|(_, width)| **width > 0)
            .max_by_key(|(_, width)| **width)
        else {
            break;
        };
        widths[index] -= 1;
    }
    spacing
}

fn value(row: &InstalledRow, column: Column) -> String {
    match column {
        Column::Name => row.name.clone(),
        Column::Application => row.application.clone(),
        Column::Arch => row.arch.clone(),
        Column::Version => row.version.clone(),
        Column::Branch => row.branch.clone(),
        Column::Runtime => row.runtime.clone(),
        Column::Ref => row.ref_name.clone(),
        Column::Origin => row.origin.clone(),
        Column::Installation => "user".to_string(),
        Column::Active => row.active.clone(),
        Column::Size => super::size_format::format(row.installed_size),
    }
}

fn write_table_line(
    output: &mut String,
    values: &[&str],
    widths: &[usize],
    spacing: usize,
    bold: bool,
) {
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            output.extend(std::iter::repeat_n(' ', spacing));
        }
        let value = truncate(value, widths[index]);
        output.push_str(&super::style::bold(&value, bold));
        if index + 1 < values.len() {
            output.extend(std::iter::repeat_n(
                ' ',
                widths[index].saturating_sub(value.chars().count()),
            ));
        }
    }
    output.push('\n');
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    if width == 0 {
        return String::new();
    }
    value
        .chars()
        .take(width - 1)
        .chain(std::iter::once('…'))
        .collect()
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
