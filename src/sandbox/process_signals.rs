use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

static SIGNAL_HANDLERS_INSTALLED: AtomicBool = AtomicBool::new(false);
pub(super) static ACTIVE_PROCESS_GROUP: AtomicI32 = AtomicI32::new(0);
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

    let process_group = ACTIVE_PROCESS_GROUP.load(Ordering::SeqCst);
    if process_group > 0 {
        unsafe {
            libc::kill(-process_group, signal);
        }
    }
}
