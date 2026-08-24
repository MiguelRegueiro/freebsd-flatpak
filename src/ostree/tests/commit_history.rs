use super::*;
use crate::installation::installation_paths::Installation;
use ostree::MutableTree;
use ostree::{ObjectType, Repo, RepoMode, RepoRemoteChange};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

fn test_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "freebsd-flatpak-history-test-{}-{}",
        std::process::id(),
        NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed)
    ))
}

#[test]
fn remote_history_fetches_tip_into_an_empty_repo_and_tolerates_pruned_tail() {
    let root = test_dir();
    fs::create_dir_all(&root).unwrap();
    let source_path = root.join("source-repo");
    let source = Repo::new_for_path(&source_path);
    source
        .create(RepoMode::Archive, gio::Cancellable::NONE)
        .unwrap();

    let tree_path = root.join("tree");
    fs::create_dir_all(&tree_path).unwrap();
    fs::write(tree_path.join("metadata"), "test").unwrap();
    let mtree = MutableTree::new();
    source
        .write_directory_to_mtree(
            &gio::File::for_path(&tree_path),
            &mtree,
            None,
            gio::Cancellable::NONE,
        )
        .unwrap();
    let transaction = source.auto_transaction(gio::Cancellable::NONE).unwrap();
    let repo_file = source
        .write_mtree(&mtree, gio::Cancellable::NONE)
        .unwrap()
        .downcast::<ostree::RepoFile>()
        .unwrap();
    let commit_metadata = VariantDict::new(None);
    commit_metadata.insert(
            "xa.metadata",
            "[Application]\nname=org.example.App\nruntime=org.example.Platform/x86_64/stable\ncommand=example\n",
        );
    let oldest = source
        .write_commit_with_time(
            None,
            Some("oldest"),
            None,
            Some(&commit_metadata.end()),
            &repo_file,
            1,
            gio::Cancellable::NONE,
        )
        .unwrap();
    let commit_metadata = VariantDict::new(None);
    commit_metadata.insert(
            "xa.metadata",
            "[Application]\nname=org.example.App\nruntime=org.example.Platform/x86_64/stable\ncommand=example\n",
        );
    let tip = source
        .write_commit_with_time(
            Some(&oldest),
            Some("tip"),
            None,
            Some(&commit_metadata.end()),
            &repo_file,
            2,
            gio::Cancellable::NONE,
        )
        .unwrap();
    let ref_name = "app/org.example.App/x86_64/stable";
    source.transaction_set_ref(None, ref_name, Some(&tip));
    transaction.commit(gio::Cancellable::NONE).unwrap();
    source
        .regenerate_summary(None, gio::Cancellable::NONE)
        .unwrap();
    let summary = fs::read(source_path.join("summary")).unwrap();

    let oldest_object = source_path
        .join("objects")
        .join(&oldest[..2])
        .join(format!("{}.commit", &oldest[2..]));
    fs::remove_file(oldest_object).unwrap();

    let paths = Installation::for_test(&root.join("destination"));
    let storage = Storage::open(&paths).unwrap();
    assert!(!storage
        .repo
        .has_object(ObjectType::Commit, &tip, gio::Cancellable::NONE)
        .unwrap());
    let remote_options = VariantDict::new(None);
    remote_options.insert("gpg-verify", false);
    remote_options.insert("gpg-verify-summary", false);
    storage
        .repo
        .remote_change(
            None::<&gio::File>,
            RepoRemoteChange::Replace,
            "test",
            Some(&format!("file://{}", source_path.display())),
            Some(&remote_options.end()),
            gio::Cancellable::NONE,
        )
        .unwrap();

    let history = storage
        .commit_history_with_verification("test", &summary, ref_name, &tip, false)
        .unwrap();

    assert_eq!(history.len(), 1);
    assert_eq!(history[0].checksum, tip);
    assert_eq!(history[0].subject, "tip");
    assert!(storage
        .repo
        .has_object(
            ObjectType::Commit,
            &history[0].checksum,
            gio::Cancellable::NONE
        )
        .unwrap());
    drop(storage);
    drop(source);
    fs::remove_dir_all(root).unwrap();
}
