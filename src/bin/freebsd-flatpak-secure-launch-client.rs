#[path = "../secure_launch.rs"]
#[allow(dead_code)]
mod secure_launch;

fn main() -> anyhow::Result<()> {
    secure_launch::run_nested_client()
}
