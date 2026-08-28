use anyhow::{anyhow, Result};

pub(crate) fn validate_partial_ref(value: &str, default_branch: Option<&str>) -> Result<()> {
    let partial = value
        .strip_prefix("app/")
        .or_else(|| value.strip_prefix("runtime/"))
        .unwrap_or(value);
    let mut parts = partial.split('/');
    let id = parts.next().unwrap_or_default();
    validate_id(id).map_err(|reason| anyhow!("Invalid id {id}: {reason}"))?;

    let embedded_branch = parts.nth(1).filter(|branch| !branch.is_empty());
    if let Some(branch) = embedded_branch.or(default_branch) {
        validate_branch(branch).map_err(|reason| anyhow!("Invalid branch {branch}: {reason}"))?;
    }
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
