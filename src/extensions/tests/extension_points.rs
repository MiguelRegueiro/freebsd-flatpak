use super::*;

fn refs(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|item| (*item).to_string()).collect()
}

#[test]
fn parses_every_generic_field_for_application_and_runtime_metadata() {
    for owner in ["Application", "Runtime"] {
        let metadata = format!(
            "[{owner}]\nname=org.example.Parent\n\
             [Extension org.example.Extension@compat]\n\
             directory=lib/extensions\nversion=stable\nversions=stable;beta;stable;\n\
             subdirectories=true\nsubdirectory-suffix=payload\nadd-ld-path=lib\n\
             merge-dirs=share/icons;share/mime;\nno-autodownload=true\n\
             download-if=active-gl-driver;on-xdg-desktop-gnome;\n\
             enable-if=active-gl-driver;\nautodelete=true\n\
             autoprune-unless=active-gl-driver;\nlocale-subset=true\n"
        );
        let points = parse_extension_points(&metadata);
        assert_eq!(points.len(), 1);
        let point = &points[0];
        assert_eq!(point.name, "org.example.Extension");
        assert_eq!(point.tag.as_deref(), Some("compat"));
        assert_eq!(point.directory.as_deref(), Some("lib/extensions"));
        assert_eq!(point.version.as_deref(), Some("stable"));
        assert_eq!(point.versions, ["stable", "beta", "stable"]);
        assert!(point.subdirectories);
        assert_eq!(point.subdirectory_suffix.as_deref(), Some("payload"));
        assert_eq!(point.add_ld_path.as_deref(), Some("lib"));
        assert_eq!(point.merge_dirs, ["share/icons", "share/mime"]);
        assert!(point.no_autodownload);
        assert_eq!(
            point.download_if,
            ["active-gl-driver", "on-xdg-desktop-gnome"]
        );
        assert_eq!(point.enable_if, ["active-gl-driver"]);
        assert!(point.autodelete);
        assert_eq!(point.autoprune_unless, ["active-gl-driver"]);
        assert!(point.locale_subset);
    }
}

#[test]
fn version_selects_one_branch() {
    let points = parse_extension_points("[Extension org.example.Extension]\nversion=42\n");
    let parent = ExtensionParent::from_ref("runtime/org.example.Platform/x86_64/99").unwrap();
    let available = refs(&[
        "runtime/org.example.Extension/x86_64/42",
        "runtime/org.example.Extension/x86_64/41",
    ]);
    assert_eq!(
        resolve_extension_refs(&points, &parent, &available),
        refs(&["runtime/org.example.Extension/x86_64/42"])
    );
}

#[test]
fn every_versions_entry_is_considered_in_declared_order() {
    let points = parse_extension_points(
        "[Extension org.example.Extension]\nversions=missing;also-missing;available;\n",
    );
    let parent = ExtensionParent::from_ref("runtime/org.example.Platform/x86_64/99").unwrap();
    let available = refs(&["runtime/org.example.Extension/x86_64/available"]);
    assert_eq!(
        resolve_extension_refs(&points, &parent, &available),
        available
    );
}

#[test]
fn application_fallback_uses_application_branch_not_runtime_branch() {
    let points = parse_extension_points("[Extension org.example.Extension]\ndirectory=lib/ext\n");
    let parent = ExtensionParent::from_ref("app/org.example.App/x86_64/beta").unwrap();
    let available = refs(&[
        "runtime/org.example.Extension/x86_64/beta",
        "runtime/org.example.Extension/x86_64/24.08",
    ]);
    assert_eq!(
        resolve_extension_refs(&points, &parent, &available),
        refs(&["runtime/org.example.Extension/x86_64/beta"])
    );
}

#[test]
fn runtime_fallback_uses_runtime_branch() {
    let points = parse_extension_points("[Extension org.example.Extension]\ndirectory=lib/ext\n");
    let parent = ExtensionParent::from_ref("org.example.Platform/x86_64/24.08").unwrap();
    let available = refs(&["runtime/org.example.Extension/x86_64/24.08"]);
    assert_eq!(
        resolve_extension_refs(&points, &parent, &available),
        available
    );
}

#[test]
fn tagged_points_share_payload_identity_and_deduplicate() {
    let points = parse_extension_points(
        "[Extension org.example.Extension@old]\nversion=42\n\
         [Extension org.example.Extension@new]\nversions=42;41\n",
    );
    let parent = ExtensionParent::from_ref("runtime/org.example.Platform/x86_64/99").unwrap();
    let available = refs(&["runtime/org.example.Extension/x86_64/42"]);
    assert_eq!(
        resolve_extension_refs(&points, &parent, &available),
        available
    );
}

#[test]
fn subdirectories_discover_all_matching_names_for_arch_and_preferred_branch() {
    let points = parse_extension_points(
        "[Extension org.example.Plugin]\nversions=2;1\nsubdirectories=true\n",
    );
    let parent = ExtensionParent::from_ref("app/org.example.App/x86_64/stable").unwrap();
    let available = refs(&[
        "runtime/org.example.Plugin.Alpha/x86_64/1",
        "runtime/org.example.Plugin.Beta/x86_64/2",
        "runtime/org.example.Plugin.Beta/x86_64/1",
        "runtime/org.example.Plugin.Other/aarch64/2",
        "runtime/org.example.Pluginish/x86_64/2",
    ]);
    assert_eq!(
        resolve_extension_refs(&points, &parent, &available),
        refs(&[
            "runtime/org.example.Plugin.Alpha/x86_64/1",
            "runtime/org.example.Plugin.Beta/x86_64/2",
        ])
    );
}

#[test]
fn architecture_isolation_and_duplicate_metadata_are_deterministic() {
    let points = parse_extension_points(
        "[Extension org.example.Extension]\nversion=42\n\
         [Extension org.example.Extension@duplicate]\nversions=42;42\n",
    );
    let parent = ExtensionParent::from_ref("runtime/org.example.Platform/x86_64/99").unwrap();
    let available = refs(&[
        "runtime/org.example.Extension/aarch64/42",
        "runtime/org.example.Extension/x86_64/42",
    ]);
    assert_eq!(
        resolve_extension_refs(&points, &parent, &available),
        refs(&["runtime/org.example.Extension/x86_64/42"])
    );
}
