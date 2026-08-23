use super::*;

fn transaction_entry() -> TransactionEntry {
    TransactionEntry {
        operation: TransactionOperation::Install,
        kind: "application",
        ref_name: "app/org.example.App/x86_64/stable".to_string(),
    }
}

#[test]
fn transaction_confirmation_accepts_enter_and_explicit_yes() {
    for answer in ["\n", "y\n", "Y\n"] {
        let mut input = std::io::Cursor::new(answer.as_bytes());
        let mut output = Vec::new();
        assert!(present_and_confirm_with(
            &[transaction_entry()],
            TransactionOptions::default(),
            &mut input,
            &mut output,
        )
        .unwrap());
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("Changes:"));
        assert!(output.contains("Proceed with these changes? [Y/n]:"));
    }
}

#[test]
fn transaction_confirmation_no_and_eof_cancel_cleanly() {
    for answer in ["n\n", "N\n", ""] {
        let mut input = std::io::Cursor::new(answer.as_bytes());
        let mut output = Vec::new();
        assert!(!present_and_confirm_with(
            &[transaction_entry()],
            TransactionOptions::default(),
            &mut input,
            &mut output,
        )
        .unwrap());
        assert!(String::from_utf8(output).unwrap().contains("Cancelled."));
    }
}

#[test]
fn assumeyes_keeps_preview_while_noninteractive_is_quiet() {
    let mut empty = std::io::Cursor::new(Vec::<u8>::new());
    let mut output = Vec::new();
    assert!(present_and_confirm_with(
        &[transaction_entry()],
        TransactionOptions {
            assumeyes: true,
            noninteractive: false,
        },
        &mut empty,
        &mut output,
    )
    .unwrap());
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("Changes:"));
    assert!(!output.contains("Proceed with"));

    let mut empty = std::io::Cursor::new(Vec::<u8>::new());
    let mut output = Vec::new();
    assert!(present_and_confirm_with(
        &[transaction_entry()],
        TransactionOptions {
            assumeyes: false,
            noninteractive: true,
        },
        &mut empty,
        &mut output,
    )
    .unwrap());
    assert_eq!(
        String::from_utf8(output).unwrap(),
        "Installing app/org.example.App/x86_64/stable\n"
    );
}
