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

    for requested in [
        "org.example.App",
        app_ref,
        "org.example.App/x86_64/stable",
        "org.example.App//stable",
        "app/org.example.App//stable",
        "app/org.example.App/x86_64",
    ] {
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

#[test]
fn installed_runtime_origin_wins_over_app_remote_for_the_same_ref() {
    let temp = std::env::temp_dir().join(format!(
        "freebsd-flatpak-resolve-installed-runtime-origin-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&temp);
    let paths = crate::installation::installation_paths::Installation::for_test(&temp);
    crate::installation::ensure_layout(&paths).unwrap();
    crate::installation::write_runtime(
        &paths,
        &crate::installation::RuntimeRecord {
            origin: "runtime-origin".to_string(),
            runtime_ref: "org.example.Platform/x86_64/stable".to_string(),
            runtime_commit: "installed-runtime".to_string(),
            explicitly_installed: false,
            installed_size: 42,
            runtime_dir: std::path::PathBuf::from("runtimes/platform/installed-runtime"),
        },
    )
    .unwrap();
    let app_ref = "app/org.example.App/x86_64/stable";
    let metadata = RemoteMetadata {
        remote: Remote {
            name: "app-origin".to_string(),
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
                checksum: "app-origin-runtime".to_string(),
                metadata: None,
                download_size: None,
                installed_size: None,
            },
        ],
        remote_dir: temp.join("remote"),
        summary_path: temp.join("summary"),
        collection_id: None,
    };

    let app = metadata
        .resolve_exact_ref_with_runtime(&paths, app_ref)
        .unwrap();
    assert_eq!(app.runtime_origin, "runtime-origin");
    assert_eq!(app.runtime_commit, "installed-runtime");
    assert_eq!(crate::installation::list_runtimes(&paths).unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(&temp);
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

#[test]
fn runtime_refs_resolve_as_first_class_refs_with_complete_identity() {
    let runtime_ref = "runtime/org.example.Platform/x86_64/50";
    let metadata = RemoteMetadata {
        remote: Remote {
            name: "runtime-origin".to_string(),
            url: "https://example.test/repo".to_string(),
            title: None,
            enabled: true,
            gpg_verify: false,
            gpg_key: None,
        },
        arch: "x86_64".to_string(),
        refs: vec![RemoteRef {
            name: runtime_ref.to_string(),
            checksum: "runtime-commit".to_string(),
            metadata: Some("[Runtime]\nname=org.example.Platform\n".to_string()),
            download_size: Some(12),
            installed_size: Some(34),
        }],
        remote_dir: std::path::PathBuf::from("/dev/null"),
        summary_path: std::path::PathBuf::from("/dev/null"),
        collection_id: None,
    };

    for requested in [
        runtime_ref,
        "org.example.Platform/x86_64/50",
        "org.example.Platform//50",
        "runtime/org.example.Platform//50",
        "runtime/org.example.Platform/x86_64",
    ] {
        let runtime = metadata.resolve_runtime(requested).unwrap();
        assert_eq!(runtime.origin, "runtime-origin");
        assert_eq!(runtime.runtime_id, "org.example.Platform");
        assert_eq!(runtime.runtime_ref, "org.example.Platform/x86_64/50");
        assert_eq!(runtime.runtime_commit, "runtime-commit");
        assert_eq!(runtime.arch, "x86_64");
        assert_eq!(runtime.branch, "50");
        assert_eq!(runtime.full_ref(), runtime_ref);
    }
}

#[test]
fn bare_runtime_id_requires_an_unambiguous_branch_when_stable_is_absent() {
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
        refs: ["49", "50"]
            .into_iter()
            .map(|branch| RemoteRef {
                name: format!("runtime/org.example.Platform/x86_64/{branch}"),
                checksum: format!("commit-{branch}"),
                metadata: None,
                download_size: None,
                installed_size: None,
            })
            .collect(),
        remote_dir: std::path::PathBuf::from("/dev/null"),
        summary_path: std::path::PathBuf::from("/dev/null"),
        collection_id: None,
    };

    let error = metadata
        .resolve_runtime("org.example.Platform")
        .unwrap_err();
    assert!(error.to_string().contains("multiple remote refs"));
}
