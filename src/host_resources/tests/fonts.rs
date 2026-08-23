use super::*;

#[test]
fn generated_font_dirs_xml_points_at_flatpak_host_paths() {
    let xml = font_dirs_xml();
    assert!(xml.contains("/run/host/fonts"));
    assert!(xml.contains("/run/host/local-fonts"));
    assert!(xml.contains("/run/host/user-fonts"));
}
