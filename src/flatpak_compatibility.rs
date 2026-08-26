use crate::flatpak_metadata::value;

/// The upstream Flatpak compatibility level exposed to applications.
///
/// This is deliberately independent of the freebsd-flatpak package version.
/// It identifies the ecosystem generation targeted by the compatibility
/// implementation; unsupported behavior is diagnosed at the concrete feature
/// boundary instead of treating this as a complete-conformance declaration.
pub(crate) const FLATPAK_COMPATIBILITY_VERSION: &str = "1.12.0";

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Version {
    major: u32,
    minor: u32,
    micro: u32,
}

pub(crate) fn required_version_diagnostic(
    metadata: &str,
    group: &str,
    reference: &str,
) -> Option<String> {
    let raw_requirements = value(metadata, group, "required-flatpak")?;
    let requirements = match raw_requirements
        .split(';')
        .map(str::trim)
        .filter(|requirement| !requirement.is_empty())
        .map(parse_version)
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(requirements) => requirements,
        Err(invalid) => {
            return Some(format!(
                "{reference} has invalid required-flatpak value {invalid:?}; attempting launch until a concrete incompatibility is encountered"
            ));
        }
    };
    if requirements.is_empty() {
        return None;
    }

    let provided = parse_version(FLATPAK_COMPATIBILITY_VERSION)
        .expect("built-in Flatpak compatibility version must be valid");
    if requirements.iter().any(|required| *required <= provided) {
        return None;
    }

    let lowest_required = requirements.into_iter().min().unwrap();
    Some(unsupported_diagnostic(reference, lowest_required))
}

fn parse_version(raw: &str) -> Result<Version, String> {
    let mut components = raw.split('.');
    let version = Version {
        major: version_component(components.next()).ok_or_else(|| raw.to_string())?,
        minor: version_component(components.next()).ok_or_else(|| raw.to_string())?,
        micro: version_component(components.next()).ok_or_else(|| raw.to_string())?,
    };
    if components.next().is_some() {
        return Err(raw.to_string());
    }
    Ok(version)
}

fn version_component(component: Option<&str>) -> Option<u32> {
    component?.parse().ok()
}

fn unsupported_diagnostic(reference: &str, required: Version) -> String {
    format!(
        "{reference} declares required-flatpak={required}, newer than the advertised compatibility level {}; attempting launch until a concrete incompatibility is encountered",
        FLATPAK_COMPATIBILITY_VERSION,
    )
}

impl std::fmt::Display for Version {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.micro)
    }
}

#[cfg(test)]
#[path = "tests/flatpak_compatibility.rs"]
mod tests;
