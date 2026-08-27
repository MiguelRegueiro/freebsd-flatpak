#[cfg(not(test))]
#[path = "../secure_mount.rs"]
#[allow(dead_code)]
mod secure_mount;

#[cfg(not(test))]
fn main() -> anyhow::Result<()> {
    secure_mount::run_helper()
}

#[cfg(test)]
fn main() {}
