use std::path::Path;

pub fn is_linux_elf(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    bytes.len() >= 20 && &bytes[0..4] == b"\x7fELF" && bytes.get(7).copied() == Some(0)
}
