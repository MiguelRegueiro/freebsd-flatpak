const DISPLAY_COMMIT_LENGTH: usize = 12;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct UpdateRow {
    pub(super) id: String,
    pub(super) branch: String,
    pub(super) kind: &'static str,
    pub(super) remote: String,
}

impl UpdateRow {
    pub(super) fn application(id: &str, branch: &str, remote: &str) -> Self {
        Self {
            id: id.to_string(),
            branch: branch.to_string(),
            kind: "Application",
            remote: remote.to_string(),
        }
    }

    pub(super) fn runtime(runtime_ref: &str, remote: &str) -> Self {
        let (id, branch) = runtime_ref
            .split_once('/')
            .and_then(|(id, remainder)| remainder.rsplit_once('/').map(|(_, branch)| (id, branch)))
            .unwrap_or((runtime_ref, ""));
        Self {
            id: id.to_string(),
            branch: branch.to_string(),
            kind: "Runtime",
            remote: remote.to_string(),
        }
    }
}

pub(super) fn render(rows: &[UpdateRow]) -> String {
    let id_width = column_width("ID", rows.iter().map(|row| row.id.as_str()));
    let branch_width = column_width("Branch", rows.iter().map(|row| row.branch.as_str()));
    let kind_width = column_width("Type", rows.iter().map(|row| row.kind));
    let mut output = format!(
        "\n        {:<id_width$}  {:<branch_width$}  {:<kind_width$}  Remote\n",
        "ID", "Branch", "Type"
    );
    for (index, row) in rows.iter().enumerate() {
        output.push_str(&format!(
            "{:>2}.     {:<id_width$}  {:<branch_width$}  {:<kind_width$}  {}\n",
            index + 1,
            row.id,
            row.branch,
            row.kind,
            row.remote
        ));
    }
    output
}

pub(super) fn short_change(
    old_ref: &str,
    new_ref: &str,
    old_commit: &str,
    new_commit: &str,
) -> String {
    let refs = if old_ref == new_ref {
        new_ref.to_string()
    } else {
        format!("{old_ref} → {new_ref}")
    };
    if old_commit == new_commit {
        format!("{refs} at {} (refresh)", display_commit(new_commit))
    } else {
        format!(
            "{refs}, {} → {}",
            display_commit(old_commit),
            display_commit(new_commit)
        )
    }
}

fn column_width<'a>(title: &str, values: impl Iterator<Item = &'a str>) -> usize {
    values
        .map(|value| value.chars().count())
        .chain([title.chars().count()])
        .max()
        .unwrap_or(0)
}

fn display_commit(commit: &str) -> &str {
    commit.get(..DISPLAY_COMMIT_LENGTH).unwrap_or(commit)
}

#[cfg(test)]
#[path = "tests/update_output.rs"]
mod tests;
