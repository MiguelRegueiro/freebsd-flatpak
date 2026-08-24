use super::*;
use crate::cli::test_support::remote_app;
use crate::ostree as storage;
use crate::remotes;

#[test]
fn remote_info_parses_log_and_historical_commit_modes() {
    assert_eq!(
        parse_remote_info_args(vec![
            "--log".to_string(),
            "flathub".to_string(),
            "org.example.App".to_string(),
        ])
        .unwrap(),
        RemoteInfoOptions {
            log: true,
            commit: None,
            app_id: "org.example.App".to_string(),
            remote: "flathub".to_string(),
        }
    );
    assert_eq!(
        parse_remote_info_args(vec![
            "--commit=abc123".to_string(),
            "flathub".to_string(),
            "org.example.App".to_string(),
        ])
        .unwrap()
        .commit
        .as_deref(),
        Some("abc123")
    );
    assert!(parse_remote_info_args(vec![
        "--log".to_string(),
        "--commit=abc123".to_string(),
        "flathub".to_string(),
        "org.example.App".to_string(),
    ])
    .is_err());
}

fn history_commit(checksum: &str, parent: Option<&str>, subject: &str) -> storage::CommitInfo {
    storage::CommitInfo {
        checksum: checksum.to_string(),
        parent: parent.map(ToString::to_string),
        timestamp: 0,
        subject: subject.to_string(),
        body: String::new(),
        flatpak_metadata: None,
        version: None,
        collection_id: None,
    }
}

#[test]
fn remote_log_matches_flatpak_metadata_and_history_structure() {
    let mut remote = remote_app(
        "org.example.App",
        "app/org.example.App/x86_64/stable",
        "tip",
    );
    remote.runtime_ref = "org.example.Platform/x86_64/50".to_string();
    remote.sdk_ref = Some("org.example.Sdk/x86_64/50".to_string());
    remote.download_size = Some(1_800_000);
    remote.installed_size = Some(4_700_000);
    let history = vec![
        history_commit("tip", Some("old"), "Current build"),
        history_commit("old", Some("unavailable"), "Older build"),
    ];
    let appstream = remotes::AppstreamInfo {
        name: Some("Example".to_string()),
        summary: Some("Do useful things".to_string()),
        version: Some("50.0".to_string()),
        license: Some("GPL-3.0-or-later".to_string()),
    };

    assert_eq!(
        remote_log_output(
            &remote,
            &history,
            Some(&appstream),
            Some("org.example.Stable"),
            false,
        ),
        concat!(
            "\n",
            "Example - Do useful things\n",
            "\n",
            "            ID: org.example.App\n",
            "           Ref: app/org.example.App/x86_64/stable\n",
            "          Arch: x86_64\n",
            "        Branch: stable\n",
            "       Version: 50.0\n",
            "       License: GPL-3.0-or-later\n",
            "    Collection: org.example.Stable\n",
            " Download Size: 1.8 MB\n",
            "Installed Size: 4.7 MB\n",
            "       Runtime: org.example.Platform/x86_64/50\n",
            "           Sdk: org.example.Sdk/x86_64/50\n",
            "\n",
            "        Commit: tip\n",
            "        Parent: old\n",
            "       Subject: Current build\n",
            "          Date: 1970-01-01 00:00:00 +0000\n",
            "       History:\n",
            "\n",
            "        Commit: old\n",
            "       Subject: Older build\n",
            "          Date: 1970-01-01 00:00:00 +0000\n",
        )
    );
}

#[test]
fn remote_info_bolds_labels_only_when_styled() {
    let remote = remote_app(
        "org.example.App",
        "app/org.example.App/x86_64/stable",
        "tip",
    );

    let plain = remote_info_output(&remote, None, None, None, false, false);
    let styled = remote_info_output(&remote, None, None, None, false, true);

    assert!(!plain.contains("\x1b["));
    assert!(styled.contains("\x1b[1mID:\x1b[0m org.example.App"));
    assert!(styled.contains("\x1b[1mCommit:\x1b[0m tip"));
    assert!(!styled.contains("\x1b[1morg.example.App"));
}

#[test]
fn remote_info_omits_header_and_optional_fields_when_metadata_is_missing() {
    let remote = remote_app(
        "org.gnome.Calculator",
        "app/org.gnome.Calculator/x86_64/stable",
        "tip",
    );

    let output = remote_info_output(&remote, None, None, None, false, false);
    assert!(output.starts_with("\n            ID: org.gnome.Calculator\n"));
    assert!(!output.contains("Calculator\n\n"));
    assert!(!output.contains("Version:"));
    assert!(!output.contains("License:"));
    assert!(!output.contains("Collection:"));
}

#[test]
fn historical_remote_info_uses_commit_version_and_collection_only() {
    let remote = remote_app(
        "org.example.App",
        "app/org.example.App/x86_64/stable",
        "old",
    );
    let appstream = remotes::AppstreamInfo {
        name: Some("Example".to_string()),
        summary: Some("Do useful things".to_string()),
        version: Some("current-version".to_string()),
        license: Some("current-license".to_string()),
    };
    let mut commit = history_commit("old", Some("older"), "Old build");
    commit.version = Some("historical-version".to_string());
    commit.collection_id = Some("org.example.Historical".to_string());

    let output = remote_info_output(
        &remote,
        Some(&commit),
        Some(&appstream),
        Some("org.example.Current"),
        true,
        false,
    );

    assert!(output.contains("Version: historical-version"));
    assert!(output.contains("Collection: org.example.Historical"));
    assert!(!output.contains("current-version"));
    assert!(!output.contains("current-license"));
    assert!(!output.contains("org.example.Current"));
}
