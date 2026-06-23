// src/signal.rs

use std::sync::atomic::{AtomicBool, Ordering};

static RELOAD_REQUESTED: AtomicBool = AtomicBool::new(false);
static SHOULD_QUIT: AtomicBool = AtomicBool::new(false);

pub fn init_signal_handler() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(unix)]
    {
        use signal_hook::consts::{SIGUSR1, SIGINT};
        use signal_hook::iterator::Signals;

        let mut signals = Signals::new(&[SIGUSR1, SIGINT])?;
        std::thread::spawn(move || {
            for sig in signals.forever() {
                match sig {
                    SIGUSR1 => {
                        RELOAD_REQUESTED.store(true, Ordering::SeqCst);
                        eprintln!("[SIGNAL] 收到 SIGUSR1，标记重载");
                    }
                    SIGINT => {
                        SHOULD_QUIT.store(true, Ordering::SeqCst);
                    }
                    _ => {}
                }
            }
        });
    }

    #[cfg(not(unix))]
    {
        eprintln!("[WARN] 当前平台不支持信号监听");
    }

    Ok(())
}

pub fn check_and_clear_reload() -> bool {
    RELOAD_REQUESTED.swap(false, Ordering::SeqCst)
}

pub fn check_and_clear_quit() -> bool {
    SHOULD_QUIT.swap(false, Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reload_flag() {
        assert!(!check_and_clear_reload());
        RELOAD_REQUESTED.store(true, Ordering::SeqCst);
        assert!(check_and_clear_reload());
        assert!(!check_and_clear_reload());
    }

    #[test]
    fn test_quit_flag() {
        assert!(!check_and_clear_quit());
        SHOULD_QUIT.store(true, Ordering::SeqCst);
        assert!(check_and_clear_quit());
        assert!(!check_and_clear_quit());
    }
}
