use std::io;
use std::os::fd::{FromRawFd, OwnedFd};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Verbosity(u8);

impl Verbosity {
    pub(crate) fn increment(&mut self) {
        self.0 = self.0.saturating_add(1);
    }

    pub(crate) fn level(self) -> u8 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Detail {
    Summary = 1,
    Detailed = 2,
}

trait Output: Send + Sync {
    fn write_line(&self, line: &str);
}

struct StderrOutput;

impl Output for StderrOutput {
    fn write_line(&self, line: &str) {
        eprintln!("{line}");
    }
}

#[derive(Clone)]
pub(crate) struct Diagnostics {
    verbosity: Verbosity,
    output: Option<Arc<dyn Output>>,
    startup_started: Option<Instant>,
}

impl Diagnostics {
    pub(crate) fn new(verbosity: Verbosity) -> Self {
        let enabled = verbosity.level() > 0;
        Self {
            verbosity,
            output: enabled.then(|| Arc::new(StderrOutput) as Arc<dyn Output>),
            startup_started: enabled.then(Instant::now),
        }
    }

    pub(crate) fn enabled(&self, detail: Detail) -> bool {
        self.verbosity.level() >= detail as u8
    }

    pub(crate) fn measure<T>(
        &self,
        detail: Detail,
        scope: &str,
        label: &str,
        operation: impl FnOnce() -> T,
    ) -> T {
        let Some(started) = self.enabled(detail).then(Instant::now) else {
            return operation();
        };
        let result = operation();
        self.write_timing(scope, label, started.elapsed());
        result
    }

    pub(crate) fn timer(&self, detail: Detail) -> Timer<'_> {
        Timer {
            diagnostics: self,
            started: self.enabled(detail).then(Instant::now),
        }
    }

    pub(crate) fn startup_complete(&self) {
        if let Some(started) = self.startup_started {
            self.write_timing("run", "startup through spawn", started.elapsed());
        }
    }

    pub(crate) fn message(&self, detail: Detail, message: impl FnOnce() -> String) {
        if self.enabled(detail) {
            self.write_line(&message());
        }
    }

    pub(crate) fn child_diagnostics(&self, detail: Detail) -> io::Result<Stdio> {
        if !self.enabled(detail) {
            return Ok(Stdio::null());
        }

        let fd = unsafe { libc::dup(libc::STDERR_FILENO) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        Ok(Stdio::from(fd))
    }

    fn write_timing(&self, scope: &str, label: &str, elapsed: Duration) {
        self.write_line(&format!(
            "{scope}: {label:<30} {:>6} ms",
            elapsed.as_millis()
        ));
    }

    fn write_line(&self, line: &str) {
        if let Some(output) = &self.output {
            output.write_line(line);
        }
    }

    #[cfg(test)]
    fn with_output(verbosity: Verbosity, output: Arc<dyn Output>) -> Self {
        let enabled = verbosity.level() > 0;
        Self {
            verbosity,
            output: enabled.then_some(output),
            startup_started: enabled.then(Instant::now),
        }
    }
}

pub(crate) struct Timer<'a> {
    diagnostics: &'a Diagnostics,
    started: Option<Instant>,
}

impl Timer<'_> {
    pub(crate) fn finish(self, scope: &str, label: &str) {
        if let Some(started) = self.started {
            self.diagnostics
                .write_timing(scope, label, started.elapsed());
        }
    }
}

#[cfg(test)]
#[path = "tests/timing.rs"]
mod tests;
