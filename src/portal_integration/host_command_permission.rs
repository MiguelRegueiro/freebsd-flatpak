use crate::flatpak_metadata;

const FLATPAK_SESSION_HELPER: &str = "org.freedesktop.Flatpak";

pub(super) fn metadata_allows_host_command(metadata: &str) -> bool {
    flatpak_metadata::section_entries(metadata, "Session Bus Policy")
        .into_iter()
        .any(|(name, policy)| name == FLATPAK_SESSION_HELPER && policy == "talk")
}

#[cfg(test)]
#[path = "tests/host_command_permission.rs"]
mod tests;
