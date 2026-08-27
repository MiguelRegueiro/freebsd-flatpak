use super::*;
use std::sync::Mutex;

#[derive(Default)]
struct CapturedOutput(Mutex<Vec<String>>);

impl Output for CapturedOutput {
    fn write_line(&self, line: &str) {
        self.0.lock().unwrap().push(line.to_string());
    }
}

fn captured(verbosity: Verbosity) -> (Diagnostics, Arc<CapturedOutput>) {
    let output = Arc::new(CapturedOutput::default());
    (Diagnostics::with_output(verbosity, output.clone()), output)
}

#[test]
fn quiet_diagnostics_do_not_run_message_formatting_or_emit_timings() {
    let (diagnostics, output) = captured(Verbosity::default());
    let mut formatted = false;
    diagnostics.message(Detail::Summary, || {
        formatted = true;
        "message".to_string()
    });
    diagnostics.measure(Detail::Summary, "run", "phase", || ());

    assert!(!formatted);
    assert!(output.0.lock().unwrap().is_empty());
}

#[test]
fn summary_and_detailed_output_follow_verbosity() {
    let mut verbose = Verbosity::default();
    verbose.increment();
    let (diagnostics, output) = captured(verbose);
    diagnostics.measure(Detail::Summary, "run", "resolve", || ());
    diagnostics.measure(Detail::Detailed, "portal", "readiness", || ());
    assert_eq!(output.0.lock().unwrap().len(), 1);
    assert!(output.0.lock().unwrap()[0].starts_with("run: resolve"));

    verbose.increment();
    let (diagnostics, output) = captured(verbose);
    diagnostics.measure(Detail::Summary, "run", "resolve", || ());
    diagnostics.measure(Detail::Detailed, "portal", "readiness", || ());
    let lines = output.0.lock().unwrap();
    assert_eq!(lines.len(), 2);
    assert!(lines[1].starts_with("portal: readiness"));
}
