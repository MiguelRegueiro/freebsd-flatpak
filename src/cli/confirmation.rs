use anyhow::Result;
use std::io::{BufRead, Write};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TransactionOptions {
    pub(crate) assumeyes: bool,
    pub(crate) noninteractive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransactionOperation {
    Install,
    Update,
    Uninstall,
}

impl TransactionOperation {
    fn label(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Update => "update",
            Self::Uninstall => "uninstall",
        }
    }

    fn active_label(self) -> &'static str {
        match self {
            Self::Install => "Installing",
            Self::Update => "Updating",
            Self::Uninstall => "Uninstalling",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransactionEntry {
    pub(crate) operation: TransactionOperation,
    pub(crate) kind: &'static str,
    pub(crate) ref_name: String,
}

pub(crate) fn present_and_confirm(
    entries: &[TransactionEntry],
    options: TransactionOptions,
) -> Result<bool> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    present_and_confirm_with(entries, options, &mut stdin.lock(), &mut stdout.lock())
}

pub(crate) fn confirm_after_preview(options: TransactionOptions) -> Result<bool> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    confirm_after_preview_with(options, &mut stdin.lock(), &mut stdout.lock())
}

pub(crate) fn present_and_confirm_with(
    entries: &[TransactionEntry],
    options: TransactionOptions,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<bool> {
    if options.noninteractive {
        for entry in entries {
            writeln!(
                output,
                "{} {}",
                entry.operation.active_label(),
                entry.ref_name
            )?;
        }
        return Ok(true);
    }

    writeln!(output, "\nChanges:")?;
    writeln!(output, "  {:<11} {:<10} Ref", "Operation", "Type")?;
    for entry in entries {
        writeln!(
            output,
            "  {:<11} {:<10} {}",
            entry.operation.label(),
            entry.kind,
            entry.ref_name
        )?;
    }
    confirm_after_preview_with(options, input, output)
}

fn confirm_after_preview_with(
    options: TransactionOptions,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<bool> {
    if options.assumeyes || options.noninteractive {
        return Ok(true);
    }

    loop {
        write!(output, "\nProceed with these changes? [Y/n]: ")?;
        output.flush()?;
        let mut answer = String::new();
        if input.read_line(&mut answer)? == 0 {
            writeln!(output, "Cancelled.")?;
            return Ok(false);
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "" | "y" | "yes" => return Ok(true),
            "n" | "no" => {
                writeln!(output, "Cancelled.")?;
                return Ok(false);
            }
            _ => writeln!(output, "Please answer y or n.")?,
        }
    }
}

#[cfg(test)]
#[path = "tests/confirmation.rs"]
mod tests;
