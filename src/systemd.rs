//! systemd service protocol (`sd_notify`) over `$NOTIFY_SOCKET`: readiness, watchdog pings
//! and stopping. Without systemd (local runs, tests) every call is a silent no-op.
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Send one notification, such as `READY=1`. Failures are logged, never fatal.
pub fn notify(state: &str) {
    let Some(socket) = std::env::var_os("NOTIFY_SOCKET") else {
        return;
    };
    if let Err(error) = send(&socket, state) {
        tracing::warn!(%error, state, "systemd notification failed");
    }
}

#[cfg(target_os = "linux")]
fn send(socket: &std::ffi::OsStr, state: &str) -> std::io::Result<()> {
    use std::os::{linux::net::SocketAddrExt, unix::net::UnixDatagram};
    let datagram = UnixDatagram::unbound()?;
    match socket.as_encoded_bytes().strip_prefix(b"@") {
        // Abstract namespace socket, as systemd usually hands out.
        Some(name) => {
            let address = std::os::unix::net::SocketAddr::from_abstract_name(name)?;
            datagram.send_to_addr(state.as_bytes(), &address)?;
        }
        None => {
            datagram.send_to(state.as_bytes(), socket)?;
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn send(_: &std::ffi::OsStr, _: &str) -> std::io::Result<()> {
    Ok(())
}

/// Ping interval for `WatchdogSec=`: a third of the deadline systemd granted this process.
fn watchdog_interval() -> Option<Duration> {
    let usec: u64 = std::env::var("WATCHDOG_USEC").ok()?.parse().ok()?;
    if let Ok(pid) = std::env::var("WATCHDOG_PID")
        && pid.parse() != Ok(std::process::id())
    {
        return None;
    }
    Some(Duration::from_micros(usec / 3).max(Duration::from_secs(1)))
}

/// Keep telling systemd the runtime is alive until `stop`. A hung runtime stops pinging,
/// and systemd restarts the service.
pub async fn watchdog(stop: CancellationToken) {
    let Some(interval) = watchdog_interval() else {
        return;
    };
    let mut ticker = tokio::time::interval(interval);
    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
            _ = ticker.tick() => notify("WATCHDOG=1"),
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::{linux::net::SocketAddrExt, unix::net::UnixDatagram};

    #[test]
    fn notifications_reach_path_and_abstract_sockets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notify.sock");
        let server = UnixDatagram::bind(&path).unwrap();
        send(path.as_os_str(), "READY=1").unwrap();
        let mut buffer = [0; 32];
        let n = server.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..n], b"READY=1");

        let name = format!("ktk-schedule-test-{}", std::process::id());
        let address = std::os::unix::net::SocketAddr::from_abstract_name(&name).unwrap();
        let server = UnixDatagram::bind_addr(&address).unwrap();
        send(std::ffi::OsStr::new(&format!("@{name}")), "WATCHDOG=1").unwrap();
        let n = server.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..n], b"WATCHDOG=1");
    }
}
