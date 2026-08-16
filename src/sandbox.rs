use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct SandboxLayout {
    pub root: PathBuf,
    pub app: PathBuf,
    pub runtime: PathBuf,
}
