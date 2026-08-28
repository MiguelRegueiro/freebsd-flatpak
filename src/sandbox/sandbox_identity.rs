use anyhow::{bail, Result};
use std::path::PathBuf;
use std::process::Command;

const DEFAULT_SHELL: &str = "/bin/sh";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SandboxIdentity {
    uid: u32,
    gid: u32,
    user_name: String,
    group_name: Option<String>,
    real_name: String,
    home_dir: PathBuf,
}

impl SandboxIdentity {
    pub(super) fn from_current_process(uid: u32, gid: u32) -> Result<Self> {
        let user_name = glib::user_name().to_string_lossy().into_owned();
        let group_name = primary_group_name();
        let real_name = glib::real_name().to_string_lossy().into_owned();
        let home_dir = glib::home_dir();

        Self::new(uid, gid, user_name, group_name, real_name, home_dir)
    }

    pub(super) fn new(
        uid: u32,
        gid: u32,
        user_name: String,
        group_name: Option<String>,
        real_name: String,
        home_dir: PathBuf,
    ) -> Result<Self> {
        validate_field("user name", &user_name)?;
        let group_name = group_name.filter(|name| !name.is_empty());
        if let Some(group_name) = &group_name {
            validate_field("group name", group_name)?;
        }
        validate_field("real name", &real_name)?;
        validate_field("home directory", &home_dir.to_string_lossy())?;
        if !home_dir.is_absolute() {
            bail!(
                "host home directory is not absolute: {}",
                home_dir.display()
            );
        }

        Ok(Self {
            uid,
            gid,
            user_name,
            group_name,
            real_name,
            home_dir,
        })
    }

    pub(super) fn uid(&self) -> u32 {
        self.uid
    }

    pub(super) fn user_name(&self) -> &str {
        &self.user_name
    }

    pub(super) fn passwd_contents(&self) -> String {
        format!(
            "{}:x:{}:{}:{}:{}:{}\nnfsnobody:x:65534:65534:Unmapped user:/:/sbin/nologin\n",
            self.user_name,
            self.uid,
            self.gid,
            self.real_name,
            self.home_dir.display(),
            DEFAULT_SHELL
        )
    }

    pub(super) fn group_contents(&self) -> String {
        let mut contents = String::new();
        if let Some(group_name) = &self.group_name {
            contents.push_str(&format!("{group_name}:x:{}:{}\n", self.gid, self.user_name));
        }
        contents.push_str("nfsnobody:x:65534:\n");
        contents
    }
}

fn primary_group_name() -> Option<String> {
    let output = Command::new("id").arg("-gn").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    Some(name)
}

fn validate_field(label: &str, value: &str) -> Result<()> {
    if value.contains([':', '\n', '\r']) {
        bail!("host {label} cannot be represented in a sandbox identity file");
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/sandbox_identity.rs"]
mod tests;
