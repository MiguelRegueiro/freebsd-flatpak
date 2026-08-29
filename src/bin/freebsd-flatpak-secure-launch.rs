#[cfg(not(test))]
#[path = "../secure_launch.rs"]
#[allow(dead_code)]
mod secure_launch;

#[cfg(not(test))]
fn main() -> anyhow::Result<()> {
    secure_launch::run_helper()
}

#[cfg(test)]
fn main() {}
