use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

static SIGNAL_HANDLERS_INSTALLED: AtomicBool = AtomicBool::new(false);
pub(super) static ACTIVE_CHILD_PID: AtomicI32 = AtomicI32::new(0);
pub(super) static LAST_SIGNAL: AtomicI32 = AtomicI32::new(0);

pub(super) fn install_signal_handlers() {
    if SIGNAL_HANDLERS_INSTALLED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    unsafe {
        libc::signal(
            libc::SIGINT,
            handle_signal as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGTERM,
            handle_signal as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGHUP,
            handle_signal as *const () as libc::sighandler_t,
        );
    }
}

extern "C" fn handle_signal(signal: libc::c_int) {
    LAST_SIGNAL.store(signal, Ordering::SeqCst);
    if signal == libc::SIGHUP {
        return;
    }

    let pid = ACTIVE_CHILD_PID.load(Ordering::SeqCst);
    if pid > 0 {
        unsafe {
            libc::kill(pid, signal);
        }
    }
}
