use super::ostree_repository::{Storage, REMOTE_NAME};
use super::CommitInfo;
use anyhow::{bail, Context, Result};
use glib::prelude::*;
use glib::VariantDict;
use ostree::{gio, ObjectType, RepoPullFlags};

impl Storage {
    pub fn commit_history(
        &self,
        summary: &[u8],
        ref_name: &str,
        tip: &str,
    ) -> Result<Vec<CommitInfo>> {
        self.commit_history_with_verification(summary, ref_name, tip, true)
    }

    pub(super) fn commit_history_with_verification(
        &self,
        summary: &[u8],
        ref_name: &str,
        tip: &str,
        gpg_verify: bool,
    ) -> Result<Vec<CommitInfo>> {
        let refs = [ref_name];
        let commits = [tip];
        let options = VariantDict::new(None);
        options.insert_value("refs", &refs.to_variant());
        options.insert_value("override-commit-ids", &commits.to_variant());
        options.insert("flags", RepoPullFlags::COMMIT_ONLY.bits() as i32);
        options.insert("depth", -1i32);
        options.insert("gpg-verify", gpg_verify);
        options.insert("gpg-verify-summary", false);
        options.insert_value("summary-bytes", &summary.to_variant());
        options.insert_value("summary-sig-bytes", &Vec::<u8>::new().to_variant());
        options.insert("n-network-retries", 5u32);
        options.insert("retry-all-network-errors", true);
        options.insert("append-user-agent", "freebsd-flatpak/0.1");

        self.repo
            .pull_with_options(REMOTE_NAME, &options.end(), None, gio::Cancellable::NONE)
            .with_context(|| format!("pull OSTree history for {ref_name}"))?;

        let mut history = Vec::new();
        let mut checksum = Some(tip.to_string());
        while let Some(current) = checksum {
            let available = self
                .repo
                .has_object(ObjectType::Commit, &current, gio::Cancellable::NONE)
                .with_context(|| format!("check for OSTree commit {current}"))?;
            if !available {
                if history.is_empty() {
                    bail!("OSTree history pull did not store tip commit {current}");
                }
                break;
            }
            let (commit, _) = self
                .repo
                .load_commit(&current)
                .with_context(|| format!("load OSTree commit {current}"))?;
            let parent = ostree::commit_get_parent(&commit).map(|value| value.to_string());
            let metadata = commit.child_value(0);
            history.push(CommitInfo {
                checksum: current,
                parent: parent.clone(),
                timestamp: ostree::commit_get_timestamp(&commit),
                subject: commit.child_value(3).str().unwrap_or_default().to_string(),
                body: commit.child_value(4).str().unwrap_or_default().to_string(),
                flatpak_metadata: variant_dict_string(&metadata, "xa.metadata"),
                version: variant_dict_string(&metadata, "xa.version"),
                collection_id: variant_dict_string(&metadata, "ostree.collection-binding")
                    .or_else(|| variant_dict_string(&metadata, "xa.collection-id")),
            });
            checksum = parent;
        }
        Ok(history)
    }
}

fn variant_dict_string(map: &glib::Variant, key: &str) -> Option<String> {
    for index in 0..map.n_children() {
        let entry = map.child_value(index);
        if entry.child_value(0).str() != Some(key) {
            continue;
        }
        return entry
            .child_value(1)
            .as_variant()
            .and_then(|value| value.str().map(ToString::to_string));
    }
    None
}

#[cfg(test)]
#[path = "tests/commit_history.rs"]
mod tests;
