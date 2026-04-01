//! Port allocation and TCP readiness utilities.

use std::{
    net::{TcpListener, TcpStream},
    time::Duration,
};

// -----------------------------------------------------------------------------
// Port Allocation
// -----------------------------------------------------------------------------

/// A held port that keeps its [`TcpListener`] open until
/// dropped, preventing TOCTOU races where another process
/// grabs the port between allocation and use.
///
/// Call [`release`] to drop the listener and obtain the raw
/// port number just before starting the server under test.
///
/// [`release`]: PortGuard::release
pub struct PortGuard {
    /// The allocated port number.
    port: u16,
    /// Held listener that prevents port reuse until dropped.
    _listener: TcpListener,
}

impl PortGuard {
    /// The allocated port number.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Consume the guard, releasing the held listener so the
    /// port can be rebound by the server under test.
    pub fn release(self) -> u16 {
        self.port
    }
}

impl std::fmt::Display for PortGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.port)
    }
}

/// Allocate a free port by binding and immediately releasing.
///
/// There is a small TOCTOU window between the release and
/// when the server binds the port. Use [`free_port_guard`]
/// to hold the port longer when you can explicitly
/// [`release`] before the server binds.
///
/// [`release`]: PortGuard::release
pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// Like [`free_port`] but returns a [`PortGuard`] that keeps
/// the listener open until dropped or [`release`]d. Useful
/// when there is meaningful setup work between port
/// allocation and server start.
///
/// [`release`]: PortGuard::release
pub fn free_port_guard() -> PortGuard {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    PortGuard {
        port,
        _listener: listener,
    }
}

// -----------------------------------------------------------------------------
// TCP Readiness
// -----------------------------------------------------------------------------

/// Block until a TCP connection to `addr` succeeds, or panic after 2 seconds.
pub fn wait_for_tcp(addr: &str) {
    for _ in 0..200 {
        if TcpStream::connect(addr).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("server at {addr} did not become ready within 2s");
}
