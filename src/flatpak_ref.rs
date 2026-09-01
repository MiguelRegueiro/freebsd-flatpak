use anyhow::{anyhow, bail, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RefKind {
    App,
    Runtime,
}

pub(crate) fn set_kind_filter(target: &mut Option<RefKind>, kind: RefKind) -> Result<()> {
    if target.is_some_and(|current| current != kind) {
        return Err(anyhow!("--app and --runtime cannot be used together"));
    }
    *target = Some(kind);
    Ok(())
}

impl RefKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Runtime => "runtime",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FlatpakRef {
    pub(crate) kind: RefKind,
    pub(crate) id: String,
    pub(crate) arch: String,
    pub(crate) branch: String,
}

impl FlatpakRef {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        let partial = PartialRef::parse(value)?;
        let kind = partial
            .kind
            .ok_or_else(|| anyhow!("full Flatpak ref has no kind: {value}"))?;
        let arch = partial
            .arch
            .ok_or_else(|| anyhow!("full Flatpak ref has no architecture: {value}"))?;
        let branch = partial
            .branch
            .ok_or_else(|| anyhow!("full Flatpak ref has no branch: {value}"))?;
        Ok(Self {
            kind,
            id: partial.id,
            arch,
            branch,
        })
    }

    pub(crate) fn full_ref(&self) -> String {
        format!(
            "{}/{}/{}/{}",
            self.kind.as_str(),
            self.id,
            self.arch,
            self.branch
        )
    }

    pub(crate) fn partial_ref(&self) -> String {
        format!("{}/{}/{}", self.id, self.arch, self.branch)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PartialRef {
    pub(crate) kind: Option<RefKind>,
    pub(crate) id: String,
    pub(crate) arch: Option<String>,
    pub(crate) branch: Option<String>,
}

impl PartialRef {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        let (kind, partial) = if let Some(value) = value.strip_prefix("app/") {
            (Some(RefKind::App), value)
        } else if let Some(value) = value.strip_prefix("runtime/") {
            (Some(RefKind::Runtime), value)
        } else {
            (None, value)
        };
        let parts = partial.split('/').collect::<Vec<_>>();
        if parts.is_empty() || parts.len() > 3 {
            bail!("invalid partial Flatpak ref: {value}");
        }
        let id = parts[0];
        validate_id(id).map_err(|reason| anyhow!("Invalid id {id}: {reason}"))?;
        let arch = parts
            .get(1)
            .filter(|value| !value.is_empty())
            .map(|value| (*value).to_string());
        if let Some(arch) = arch.as_deref() {
            validate_arch(arch)
                .map_err(|reason| anyhow!("Invalid architecture {arch}: {reason}"))?;
        }
        let branch = parts
            .get(2)
            .filter(|value| !value.is_empty())
            .map(|value| (*value).to_string());
        if let Some(branch) = branch.as_deref() {
            validate_branch(branch)
                .map_err(|reason| anyhow!("Invalid branch {branch}: {reason}"))?;
        }
        Ok(Self {
            kind,
            id: id.to_string(),
            arch,
            branch,
        })
    }

    pub(crate) fn effective_kind(&self, filter: Option<RefKind>) -> Result<Option<RefKind>> {
        if let (Some(prefix), Some(filter)) = (self.kind, filter) {
            if prefix != filter {
                bail!(
                    "{} ref cannot be used with --{}",
                    prefix.as_str(),
                    filter.as_str()
                );
            }
        }
        Ok(self.kind.or(filter))
    }

    pub(crate) fn matches(&self, candidate: &FlatpakRef) -> bool {
        self.kind.is_none_or(|kind| kind == candidate.kind)
            && self.id == candidate.id
            && self
                .arch
                .as_deref()
                .is_none_or(|arch| arch == candidate.arch)
            && self
                .branch
                .as_deref()
                .is_none_or(|branch| branch == candidate.branch)
    }

    pub(crate) fn with_default_branch(mut self, branch: Option<&str>) -> Result<Self> {
        if self.branch.is_none() {
            if let Some(branch) = branch {
                validate_branch(branch)
                    .map_err(|reason| anyhow!("Invalid branch {branch}: {reason}"))?;
                self.branch = Some(branch.to_string());
            }
        }
        Ok(self)
    }
}

pub(crate) fn validate_partial_ref(value: &str, default_branch: Option<&str>) -> Result<()> {
    PartialRef::parse(value)?.with_default_branch(default_branch)?;
    Ok(())
}

fn validate_id(id: &str) -> std::result::Result<(), String> {
    if id.is_empty() {
        return Err("Name can't be empty".to_string());
    }
    if id.len() > 255 {
        return Err("Name can't be longer than 255 characters".to_string());
    }

    let period_count = id.bytes().filter(|byte| *byte == b'.').count();
    for (segment_index, segment) in id.split('.').enumerate() {
        if segment.is_empty() {
            return if segment_index == 0 {
                Err("Name can't start with a period".to_string())
            } else if id.ends_with('.') {
                Err("Name can't end with a period".to_string())
            } else {
                Err("Name segment can't start with \".\"".to_string())
            };
        }

        let last_segment = period_count > 0 && segment_index == period_count;
        let mut characters = segment.chars();
        let first = characters.next().unwrap();
        if !valid_initial_id_char(first, last_segment) {
            if first == '-' {
                return Err("Only last name segment can contain -".to_string());
            }
            let description = describe_char(first);
            return Err(if segment_index == 0 {
                format!("Name can't start with \"{description}\"")
            } else {
                format!("Name segment can't start with \"{description}\"")
            });
        }
        for character in characters {
            if !valid_id_char(character, last_segment) {
                if character == '-' {
                    return Err("Only last name segment can contain -".to_string());
                }
                return Err(format!(
                    "Name can't contain \"{}\"",
                    describe_char(character)
                ));
            }
        }
    }

    if period_count < 2 {
        return Err("Names must contain at least 2 periods".to_string());
    }
    Ok(())
}

fn valid_initial_id_char(character: char, allow_dash: bool) -> bool {
    character.is_ascii_alphabetic() || character == '_' || (allow_dash && character == '-')
}

fn valid_id_char(character: char, allow_dash: bool) -> bool {
    valid_initial_id_char(character, allow_dash) || character.is_ascii_digit()
}

fn validate_arch(arch: &str) -> std::result::Result<(), String> {
    if arch
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        Ok(())
    } else {
        Err("Architecture contains invalid characters".to_string())
    }
}

fn validate_branch(branch: &str) -> std::result::Result<(), String> {
    if branch.is_empty() {
        return Err("Branch can't be empty".to_string());
    }
    let mut characters = branch.chars();
    let first = characters.next().unwrap();
    if !valid_initial_branch_char(first) {
        return Err(format!(
            "Branch can't start with \"{}\"",
            describe_char(first)
        ));
    }
    if let Some(invalid) = characters.find(|character| !valid_branch_char(*character)) {
        return Err(format!(
            "Branch can't contain \"{}\"",
            describe_char(invalid)
        ));
    }
    Ok(())
}

fn valid_initial_branch_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
}

fn valid_branch_char(character: char) -> bool {
    valid_initial_branch_char(character) || character == '.'
}

fn describe_char(character: char) -> String {
    if character.is_control() {
        format!("U+{:04X}", character as u32)
    } else {
        character.to_string()
    }
}

#[cfg(test)]
#[path = "tests/flatpak_ref.rs"]
mod tests;
