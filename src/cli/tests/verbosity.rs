use super::parse_global_options;

fn parse(args: &[&str]) -> (u8, Vec<String>) {
    let (verbosity, remaining) =
        parse_global_options(args.iter().map(|arg| (*arg).to_string()).collect());
    (verbosity.level(), remaining)
}

#[test]
fn default_verbosity_is_quiet() {
    assert_eq!(
        parse(&["run", "org.example.App"]),
        (0, vec!["run".to_string(), "org.example.App".to_string()])
    );
}

#[test]
fn parses_single_verbose_flag() {
    assert_eq!(parse(&["-v", "run", "org.example.App"]).0, 1);
}

#[test]
fn parses_compact_double_verbose_flag() {
    assert_eq!(parse(&["-vv", "run", "org.example.App"]).0, 2);
}

#[test]
fn repeated_and_long_verbose_flags_accumulate() {
    assert_eq!(parse(&["-v", "--verbose", "run", "org.example.App"]).0, 2);
}

#[test]
fn verbosity_parsing_stops_at_the_command() {
    assert_eq!(
        parse(&["run", "org.example.App", "-v"]),
        (
            0,
            vec![
                "run".to_string(),
                "org.example.App".to_string(),
                "-v".to_string()
            ]
        )
    );
}
