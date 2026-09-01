use anyhow::{bail, Context, Result};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FlatpakArchitecture {
    X86_64,
    Aarch64,
}

impl FlatpakArchitecture {
    pub(crate) fn from_flatpak_name(name: &str) -> Result<Self> {
        match name {
            "x86_64" => Ok(Self::X86_64),
            "aarch64" => Ok(Self::Aarch64),
            _ => bail!("unsupported Flatpak architecture: {name}"),
        }
    }

    pub(crate) fn from_host_machine(machine: &str) -> Result<Self> {
        match machine {
            "amd64" | "x86_64" => Ok(Self::X86_64),
            "arm64" | "aarch64" => Ok(Self::Aarch64),
            _ => bail!("unsupported host architecture for Flatpak: {machine}"),
        }
    }

    pub(crate) fn from_runtime_ref(runtime_ref: &str) -> Result<Self> {
        let arch = runtime_ref
            .split('/')
            .nth(1)
            .context("missing runtime arch")?;
        Self::from_flatpak_name(arch)
    }

    pub(crate) fn host() -> Result<Self> {
        let output = Command::new("uname")
            .arg("-m")
            .output()
            .context("determine host architecture")?;
        if !output.status.success() {
            bail!("uname -m failed with status {}", output.status);
        }
        let machine = String::from_utf8(output.stdout)?;
        Self::from_host_machine(machine.trim())
    }

    pub(crate) fn flatpak_name(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }

    pub(crate) fn linux_multiarch_tuple(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64-linux-gnu",
            Self::Aarch64 => "aarch64-linux-gnu",
        }
    }

    pub(crate) fn runtime_libdir(self) -> String {
        format!("lib/{}", self.linux_multiarch_tuple())
    }

    pub(crate) fn vulkan_icd_filename(self, driver: &str) -> String {
        format!("{driver}_icd.{}.json", self.flatpak_name())
    }
}

#[cfg(test)]
#[path = "tests/architecture.rs"]
mod tests;
