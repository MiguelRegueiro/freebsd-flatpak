use super::*;
use crate::sandbox::filesystem_permissions::AccessMode;

fn grant(source: &str, host_path: &Path, sandbox_path: &Path) -> HostPathGrant {
    HostPathGrant::new(
        source,
        source,
        host_path.to_path_buf(),
        sandbox_path.to_path_buf(),
        AccessMode::ReadWrite,
    )
    .unwrap()
}

#[test]
fn broad_home_grant_is_mounted_before_private_app_data_root() {
    let home = Path::new("/home/user");
    let app_data = home.join(".var/app/org.example.App");
    let home_grant = grant("host", home, home);

    let plan = AppDataMountPlan::build_from_parts(&[home_grant], &[], &app_data).unwrap();

    assert_eq!(plan.grants_before_app_data.len(), 1);
    assert!(plan.mask_app_data_root);
    assert_eq!(plan.app_data_root, home.join(".var/app"));
    assert!(plan.grants_inside_app_data_root.is_empty());
}

#[test]
fn explicit_cross_app_grant_is_reapplied_inside_private_app_data_root() {
    let home = Path::new("/home/user");
    let app_data_root = home.join(".var/app");
    let app_data = app_data_root.join("org.example.App");
    let other_profile = app_data_root.join("org.example.Other/data/profile");
    let grants = [
        grant("host", home, home),
        grant(
            "~/.var/app/org.example.Other/data/profile",
            &other_profile,
            &other_profile,
        ),
    ];

    let plan = AppDataMountPlan::build_from_parts(&grants, &[], &app_data).unwrap();

    assert_eq!(plan.grants_before_app_data.len(), 1);
    assert!(plan.mask_app_data_root);
    assert_eq!(plan.grants_inside_app_data_root.len(), 1);
    assert_eq!(
        plan.grants_inside_app_data_root[0].sandbox_path(),
        other_profile
    );
}

#[test]
fn persistent_home_covering_app_data_root_gets_the_same_private_boundary() {
    let home = Path::new("/home/user");
    let app_data = home.join(".var/app/org.example.App");

    let all_home =
        AppDataMountPlan::build_from_parts(&[], &[PathBuf::from(".")], &app_data).unwrap();
    let unrelated =
        AppDataMountPlan::build_from_parts(&[], &[PathBuf::from(".example")], &app_data).unwrap();

    assert!(all_home.mask_app_data_root);
    assert!(!unrelated.mask_app_data_root);
}

#[test]
fn app_without_covering_home_mount_needs_no_mask() {
    let home = Path::new("/home/user");
    let app_data = home.join(".var/app/org.example.App");
    let downloads = home.join("Downloads");

    let plan = AppDataMountPlan::build_from_parts(
        &[grant("xdg-download", &downloads, &downloads)],
        &[],
        &app_data,
    )
    .unwrap();

    assert!(!plan.mask_app_data_root);
    assert_eq!(plan.grants_before_app_data.len(), 1);
    assert!(plan.grants_inside_app_data_root.is_empty());
}
