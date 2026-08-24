use super::*;

#[test]
fn decimal_format_matches_glib_flatpak_ui() {
    assert_eq!(format(402_500_000), "402.5 MB");
    assert_eq!(format(1_100_000_000), "1.1 GB");
    assert_eq!(format(789_500), "789.5 kB");
}
