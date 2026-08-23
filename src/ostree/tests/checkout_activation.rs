use super::*;
use crate::ostree::ostree_repository::TRANSACTION_FILE;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

fn test_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "freebsd-flatpak-storage-test-{}-{}",
        std::process::id(),
        NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed)
    ))
}

fn deployment(path: &Path, value: &str) {
    fs::create_dir_all(path).unwrap();
    fs::write(path.join("value"), value).unwrap();
}

#[test]
fn recovery_finishes_every_staged_activation() {
    let root = test_dir();
    fs::create_dir_all(&root).unwrap();
    let transaction = root.join(TRANSACTION_FILE);

    let first = root.join("first");
    let first_staging = root.join(".first.staging");
    let first_backup = root.join(".first.previous");
    deployment(&first, "new-first");
    deployment(&first_backup, "old-first");

    let second = root.join("second");
    let second_staging = root.join(".second.staging");
    let second_backup = root.join(".second.previous");
    deployment(&second, "old-second");
    deployment(&second_staging, "new-second");

    fs::write(
        &transaction,
        format!(
            "1\n{}\t{}\t{}\n{}\t{}\t{}\n",
            first.display(),
            first_staging.display(),
            first_backup.display(),
            second.display(),
            second_staging.display(),
            second_backup.display()
        ),
    )
    .unwrap();

    recover_activation_file(&transaction).unwrap();

    assert_eq!(
        fs::read_to_string(first.join("value")).unwrap(),
        "new-first"
    );
    assert_eq!(
        fs::read_to_string(second.join("value")).unwrap(),
        "new-second"
    );
    assert!(!first_backup.exists());
    assert!(!second_backup.exists());
    assert!(!transaction.exists());
    fs::remove_dir_all(root).unwrap();
}
