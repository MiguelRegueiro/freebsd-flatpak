use super::{
    remote_app_from_metadata, resolve_current_app_id_from_replacements, select_history_commit,
};
use crate::ostree::CommitInfo;
use crate::remotes::{Remote, RemoteMetadata, RemoteRef};
use std::collections::BTreeMap;

#[test]
fn exact_app_ref_does_not_require_appstream_metadata() {
    let metadata = RemoteMetadata {
        remote: Remote {
            name: "test".to_string(),
            url: "https://example.test/repo".to_string(),
            title: None,
            enabled: true,
            gpg_verify: false,
            gpg_key: None,
        },
        arch: "x86_64".to_string(),
        refs: vec![RemoteRef {
            name: "app/org.example.App/x86_64/stable".to_string(),
            checksum: "app-commit".to_string(),
            metadata: None,
            download_size: None,
            installed_size: None,
        }],
        remote_dir: std::path::PathBuf::from("/dev/null"),
        summary_path: std::path::PathBuf::from("/dev/null"),
        collection_id: None,
    };

    let remote_ref = metadata.resolve_app_ref("org.example.App", true).unwrap();
    assert_eq!(remote_ref.name, "app/org.example.App/x86_64/stable");
}

#[test]
fn named_remote_resolution_records_origin_and_accepts_full_refs() {
    let app_ref = "app/org.example.App/x86_64/stable";
    let metadata = RemoteMetadata {
        remote: Remote {
            name: "example".to_string(),
            url: "https://example.test/repo".to_string(),
            title: None,
            enabled: true,
            gpg_verify: false,
            gpg_key: None,
        },
        arch: "x86_64".to_string(),
        refs: vec![
            RemoteRef {
                name: app_ref.to_string(),
                checksum: "app-commit".to_string(),
                metadata: Some("[Application]\nname=org.example.App\nruntime=org.example.Platform/x86_64/stable\ncommand=example\n".to_string()),
                download_size: None,
                installed_size: None,
            },
            RemoteRef {
                name: "runtime/org.example.Platform/x86_64/stable".to_string(),
                checksum: "runtime-commit".to_string(),
                metadata: None,
                download_size: None,
                installed_size: None,
            },
        ],
        remote_dir: std::path::PathBuf::from("/dev/null"),
        summary_path: std::path::PathBuf::from("/dev/null"),
        collection_id: None,
    };

    for requested in ["org.example.App", app_ref, "org.example.App/x86_64/stable"] {
        let app = metadata.resolve_app(requested, false).unwrap();
        assert_eq!(app.origin, "example");
        assert_eq!(app.runtime_origin, "example");
    }
}

#[test]
fn cross_remote_runtime_origin_is_preserved() {
    let app = remote_app_from_metadata(
        RemoteRef {
            name: "app/org.example.App/x86_64/stable".to_string(),
            checksum: "app".to_string(),
            metadata: None,
            download_size: None,
            installed_size: None,
        },
        "[Application]\nname=org.example.App\nruntime=org.example.Platform/x86_64/stable\ncommand=example\n".to_string(),
        RemoteRef {
            name: "runtime/org.example.Platform/x86_64/stable".to_string(),
            checksum: "runtime".to_string(),
            metadata: None,
            download_size: None,
            installed_size: None,
        },
        "x86_64",
        "apps",
        "runtimes",
    )
    .unwrap();
    assert_eq!(app.origin, "apps");
    assert_eq!(app.runtime_origin, "runtimes");
}

fn commit(checksum: &str) -> CommitInfo {
    CommitInfo {
        checksum: checksum.to_string(),
        parent: None,
        timestamp: 0,
        subject: String::new(),
        body: String::new(),
        flatpak_metadata: None,
        version: None,
        collection_id: None,
    }
}

#[test]
fn historical_commit_selection_accepts_unique_prefixes() {
    let history = vec![
        commit("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        commit("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
    ];

    assert_eq!(
        select_history_commit(&history, "BBBBBBBBBBBB")
            .unwrap()
            .checksum,
        history[1].checksum
    );
}

#[test]
fn historical_commit_selection_rejects_commits_outside_ref_history() {
    let history = vec![commit(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )];

    let error = select_history_commit(&history, "bbbbbbbbbbbb").unwrap_err();
    assert!(error.to_string().contains("not in the history"));
}

#[test]
fn current_app_id_follows_available_replacement() {
    let refs = vec![RemoteRef {
        name: "app/app.example.Current/x86_64/stable".to_string(),
        checksum: "app-2".to_string(),
        metadata: None,
        download_size: None,
        installed_size: None,
    }];
    let replacements = BTreeMap::from([(
        "org.example.Old".to_string(),
        vec!["app.example.Current".to_string()],
    )]);

    assert_eq!(
        resolve_current_app_id_from_replacements(&refs, &replacements, "org.example.Old", "x86_64")
            .unwrap(),
        "app.example.Current"
    );
}

#[test]
fn current_app_id_ignores_unavailable_replacement() {
    let refs = vec![RemoteRef {
        name: "app/app.example.Current/aarch64/stable".to_string(),
        checksum: "app-2".to_string(),
        metadata: None,
        download_size: None,
        installed_size: None,
    }];
    let replacements = BTreeMap::from([(
        "org.example.Old".to_string(),
        vec!["app.example.Current".to_string()],
    )]);

    assert_eq!(
        resolve_current_app_id_from_replacements(&refs, &replacements, "org.example.Old", "x86_64")
            .unwrap(),
        "org.example.Old"
    );
}

#[test]
fn current_app_id_rejects_ambiguous_replacements() {
    let refs = vec![
        RemoteRef {
            name: "app/app.example.One/x86_64/stable".to_string(),
            checksum: "app-1".to_string(),
            metadata: None,
            download_size: None,
            installed_size: None,
        },
        RemoteRef {
            name: "app/app.example.Two/x86_64/stable".to_string(),
            checksum: "app-2".to_string(),
            metadata: None,
            download_size: None,
            installed_size: None,
        },
    ];
    let replacements = BTreeMap::from([(
        "org.example.Old".to_string(),
        vec!["app.example.One".to_string(), "app.example.Two".to_string()],
    )]);

    let error =
        resolve_current_app_id_from_replacements(&refs, &replacements, "org.example.Old", "x86_64")
            .unwrap_err();

    assert!(error.to_string().contains("multiple replacements found"));
}

#[test]
fn current_app_id_rejects_replacement_cycles() {
    let refs = vec![
        RemoteRef {
            name: "app/org.example.A/x86_64/stable".to_string(),
            checksum: "app-a".to_string(),
            metadata: None,
            download_size: None,
            installed_size: None,
        },
        RemoteRef {
            name: "app/org.example.B/x86_64/stable".to_string(),
            checksum: "app-b".to_string(),
            metadata: None,
            download_size: None,
            installed_size: None,
        },
    ];
    let replacements = BTreeMap::from([
        (
            "org.example.A".to_string(),
            vec!["org.example.B".to_string()],
        ),
        (
            "org.example.B".to_string(),
            vec!["org.example.A".to_string()],
        ),
    ]);

    let error =
        resolve_current_app_id_from_replacements(&refs, &replacements, "org.example.A", "x86_64")
            .unwrap_err();

    assert!(error.to_string().contains("cycle in replacement metadata"));
}
