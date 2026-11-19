//! Process-exit indirection for embedded hosts.
//!
//! The CLI is also a library: `create` / `build_module_command` and the
//! dispatch entry points are called from host applications that own the
//! process. A hard `std::process::exit()` in those paths makes the host
//! untestable. Every library-side exit goes through [`exit_with`]; the default
//! still calls `process::exit`, so the standalone binary is unchanged.
//!
//! ```no_run
//! apcore_cli::set_exit_handler(Some(std::sync::Arc::new(|code| {
//!     // record the code, unwind, or map it to the host's own exit path
//!     std::process::exit(code)
//! })));
//! ```

use std::sync::{Arc, LazyLock, Mutex};

/// Receives the exit code a library path wants to terminate with.
pub type ExitHandler = Arc<dyn Fn(i32) -> ! + Send + Sync>;

static EXIT_HANDLER: LazyLock<Mutex<Option<ExitHandler>>> = LazyLock::new(|| Mutex::new(None));

/// Replace the process-exit action used by the library.
///
/// Pass `None` to restore the default (`std::process::exit`).
pub fn set_exit_handler(handler: Option<ExitHandler>) {
    if let Ok(mut guard) = EXIT_HANDLER.lock() {
        *guard = handler;
    }
}

/// The currently installed exit handler, if any.
pub fn get_exit_handler() -> Option<ExitHandler> {
    EXIT_HANDLER.lock().ok().and_then(|guard| guard.clone())
}

/// Terminate through the installed handler (default: `std::process::exit`).
pub fn exit_with(code: i32) -> ! {
    if let Some(handler) = get_exit_handler() {
        handler(code);
    }
    std::process::exit(code);
}
