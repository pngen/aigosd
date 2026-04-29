use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn reset_shutdown() {
    SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
}

pub fn shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
}

pub fn install_shutdown_handler() -> io::Result<()> {
    platform::install_shutdown_handler()
}

#[cfg(unix)]
mod platform {
    use super::SHUTDOWN_REQUESTED;
    use std::io;
    use std::sync::atomic::Ordering;

    const SIGINT: i32 = 2;
    const SIGTERM: i32 = 15;
    const SIG_ERR: usize = usize::MAX;

    extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }

    extern "C" fn handle_signal(_signal: i32) {
        SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
    }

    pub fn install_shutdown_handler() -> io::Result<()> {
        unsafe {
            if signal(SIGINT, handle_signal as *const () as usize) == SIG_ERR {
                return Err(io::Error::last_os_error());
            }
            if signal(SIGTERM, handle_signal as *const () as usize) == SIG_ERR {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
mod platform {
    use super::SHUTDOWN_REQUESTED;
    use std::io;
    use std::sync::atomic::Ordering;

    const CTRL_C_EVENT: u32 = 0;
    const CTRL_BREAK_EVENT: u32 = 1;
    const CTRL_CLOSE_EVENT: u32 = 2;
    const CTRL_LOGOFF_EVENT: u32 = 5;
    const CTRL_SHUTDOWN_EVENT: u32 = 6;

    extern "system" {
        fn SetConsoleCtrlHandler(handler: Option<extern "system" fn(u32) -> i32>, add: i32) -> i32;
    }

    extern "system" fn handle_ctrl_event(event: u32) -> i32 {
        match event {
            CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT
            | CTRL_SHUTDOWN_EVENT => {
                SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
                1
            }
            _ => 0,
        }
    }

    pub fn install_shutdown_handler() -> io::Result<()> {
        let ok = unsafe { SetConsoleCtrlHandler(Some(handle_ctrl_event), 1) };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}
