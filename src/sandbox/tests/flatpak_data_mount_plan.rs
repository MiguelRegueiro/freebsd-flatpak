use super::*;
use crate::sandbox::filesystem_permissions::AccessMode;

fn grant(
    permission: &str,
    host_path: &Path,
    sandbox_path: &Path,
    access: AccessMode,
) -> HostPathGrant {
    HostPathGrant::new(
        permission,
        permission,
        host_path.to_path_buf(),
        sandbox_path.to_path_buf(),
        access,
    )
    .unwrap()
}

fn app_paths() -> (PathBuf, PathBuf) {
    let root = PathBuf::from("/home/user/.var/app");
    let own = root.join("org.example.App");
    (root, own)
}

#[test]
fn host_plus_cross_app_grant_masks_then_restores_only_requested_subtree() {
    let (root, own) = app_paths();
    let steam = root.join("com.valvesoftware.Steam/data/Steam");
    let plan = FlatpakDataMountPlan::build_from_grants(
        &[
            grant(
                "host",
                Path::new("/home/user"),
                Path::new("/home/user"),
                AccessMode::ReadWrite,
            ),
            grant(
                "~/.var/app/com.valvesoftware.Steam/data/Steam",
                &steam,
                &steam,
                AccessMode::ReadWrite,
            ),
        ],
        &own,
    )
    .unwrap();

    assert!(plan.mask_app_data_root);
    assert_eq!(plan.grants_before_mask.len(), 1);
    assert_eq!(plan.grants_after_mask.len(), 1);
    assert_eq!(plan.grants_after_mask[0].sandbox_path(), steam);
    assert_eq!(plan.app_data_root, root);
}

#[test]
fn home_plus_cross_app_grant_uses_the_same_private_boundary() {
    let (root, own) = app_paths();
    let other = root.join("org.example.Other/data");
    let plan = FlatpakDataMountPlan::build_from_grants(
        &[
            grant(
                "home",
                Path::new("/home/user"),
                Path::new("/home/user"),
                AccessMode::ReadWrite,
            ),
            grant(
                "~/.var/app/org.example.Other/data",
                &other,
                &other,
                AccessMode::ReadWrite,
            ),
        ],
        &own,
    )
    .unwrap();

    assert!(plan.mask_app_data_root);
    assert_eq!(plan.grants_after_mask.len(), 1);
}

#[test]
fn broad_access_does_not_become_an_implicit_cross_app_grant() {
    let (_, own) = app_paths();
    let plan = FlatpakDataMountPlan::build_from_grants(
        &[grant(
            "host",
            Path::new("/home"),
            Path::new("/home"),
            AccessMode::ReadWrite,
        )],
        &own,
    )
    .unwrap();

    assert!(plan.mask_app_data_root);
    assert!(plan.grants_after_mask.is_empty());
}

#[test]
fn own_app_data_is_always_restored_writable_by_the_backend() {
    let (_, own) = app_paths();
    let plan = FlatpakDataMountPlan::build_from_grants(&[], &own).unwrap();

    assert_eq!(plan.app_data, own);
    assert!(!plan.mask_app_data_root);
}

#[test]
fn redundant_own_app_child_grant_cannot_form_a_self_alias() {
    let (_, own) = app_paths();
    let child = own.join("data/tools");
    let plan = FlatpakDataMountPlan::build_from_grants(
        &[grant(
            "~/.var/app/org.example.App/data/tools",
            &child,
            &child,
            AccessMode::ReadWrite,
        )],
        &own,
    )
    .unwrap();

    assert!(plan.grants_after_mask.is_empty());
}

#[test]
fn canonical_symlink_source_keeps_lexical_cross_app_destination() {
    let (root, own) = app_paths();
    let lexical = root.join("com.valvesoftware.Steam/data/Steam");
    let canonical = root.join("com.valvesoftware.Steam/.local/share/Steam");
    let plan = FlatpakDataMountPlan::build_from_grants(
        &[grant(
            "~/.var/app/com.valvesoftware.Steam/data/Steam",
            &canonical,
            &lexical,
            AccessMode::ReadWrite,
        )],
        &own,
    )
    .unwrap();

    let restored = &plan.grants_after_mask[0];
    assert_eq!(restored.host_path(), canonical);
    assert_eq!(restored.sandbox_path(), lexical);
    assert_ne!(restored.host_path(), restored.sandbox_path());
}

#[test]
fn explicit_cross_app_read_only_access_is_preserved() {
    let (root, own) = app_paths();
    let other = root.join("org.example.Other/data");
    let plan = FlatpakDataMountPlan::build_from_grants(
        &[grant(
            "~/.var/app/org.example.Other/data:ro",
            &other,
            &other,
            AccessMode::ReadOnly,
        )],
        &own,
    )
    .unwrap();

    assert_eq!(plan.grants_after_mask[0].access(), AccessMode::ReadOnly);
}

#[test]
fn ordinary_host_grants_remain_before_the_private_boundary() {
    let (_, own) = app_paths();
    let documents = Path::new("/home/user/Documents");
    let plan = FlatpakDataMountPlan::build_from_grants(
        &[grant("host", documents, documents, AccessMode::ReadWrite)],
        &own,
    )
    .unwrap();

    assert_eq!(plan.grants_before_mask.len(), 1);
    assert!(!plan.mask_app_data_root);
}

#[test]
fn broad_access_masks_flatpak_installation_storage_too() {
    let (_, own) = app_paths();
    let installation_roots = [
        PathBuf::from("/home/user/.local/share/flatpak"),
        PathBuf::from("/home/user/.local/share/freebsd-flatpak"),
    ];
    let plan = FlatpakDataMountPlan::build_from_parts(
        &[grant(
            "host",
            Path::new("/home/user"),
            Path::new("/home/user"),
            AccessMode::ReadWrite,
        )],
        &own,
        &installation_roots,
    )
    .unwrap();

    assert_eq!(plan.reserved_roots_to_mask, installation_roots);
}

#[test]
fn every_protected_root_regrants_exact_child_and_deep_paths_after_masks() {
    let (app_root, own) = app_paths();
    let flatpak_root = PathBuf::from("/home/user/.local/share/flatpak");
    let project_root = PathBuf::from("/home/user/.local/share/freebsd-flatpak");
    let protected_roots = [app_root.clone(), flatpak_root.clone(), project_root.clone()];
    let grants = vec![
        grant("app-root:rw", &app_root, &app_root, AccessMode::ReadWrite),
        grant(
            "exact:ro",
            &flatpak_root,
            &flatpak_root,
            AccessMode::ReadOnly,
        ),
        grant(
            "child:rw",
            &project_root.join("child"),
            &project_root.join("child"),
            AccessMode::ReadWrite,
        ),
        grant(
            "deep:create",
            &app_root.join("org.example.Other/a/b/c"),
            &app_root.join("org.example.Other/a/b/c"),
            AccessMode::ReadWrite,
        ),
        grant(
            "ordinary",
            Path::new("/home/user/Documents"),
            Path::new("/home/user/Documents"),
            AccessMode::ReadWrite,
        ),
    ];

    let plan =
        FlatpakDataMountPlan::build_from_parts(&grants, &own, &protected_roots[1..]).unwrap();

    assert_eq!(plan.grants_before_mask.len(), 1);
    assert_eq!(
        plan.grants_before_mask[0].sandbox_path(),
        Path::new("/home/user/Documents")
    );
    assert_eq!(plan.grants_after_mask.len(), 4);
    assert_eq!(
        plan.grants_after_mask
            .iter()
            .map(HostPathGrant::source_permission)
            .collect::<Vec<_>>(),
        vec!["app-root:rw", "exact:ro", "child:rw", "deep:create"]
    );
    assert_eq!(
        plan.grants_after_mask
            .iter()
            .map(HostPathGrant::access)
            .collect::<Vec<_>>(),
        vec![
            AccessMode::ReadWrite,
            AccessMode::ReadOnly,
            AccessMode::ReadWrite,
            AccessMode::ReadWrite,
        ]
    );
}

#[test]
fn current_app_canonical_mount_stays_separate_from_generic_regrants() {
    let (app_root, own) = app_paths();
    let own_child = own.join("data/tools");
    let other = app_root.join("org.example.Other");
    let plan = FlatpakDataMountPlan::build_from_parts(
        &[
            grant("own:create", &own_child, &own_child, AccessMode::ReadWrite),
            grant("other:ro", &other, &other, AccessMode::ReadOnly),
        ],
        &own,
        &[],
    )
    .unwrap();

    assert_eq!(plan.app_data, own);
    assert_eq!(plan.grants_after_mask.len(), 1);
    assert_eq!(plan.grants_after_mask[0].sandbox_path(), other);
}

#[test]
fn ordinary_app_without_host_or_home_creates_no_protected_masks() {
    let (_, own) = app_paths();
    let reserved = [
        PathBuf::from("/home/user/.local/share/flatpak"),
        PathBuf::from("/home/user/.local/share/freebsd-flatpak"),
    ];
    let plan = FlatpakDataMountPlan::build_from_parts(&[], &own, &reserved).unwrap();

    assert!(!plan.mask_app_data_root);
    assert!(plan.reserved_roots_to_mask.is_empty());
    assert!(plan.grants_before_mask.is_empty());
    assert!(plan.grants_after_mask.is_empty());
}
