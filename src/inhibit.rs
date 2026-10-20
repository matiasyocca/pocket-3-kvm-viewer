//! RAII ownership of a systemd-logind idle/sleep inhibitor.
//!
//! The lock is represented by an owned descriptor, not a separate release RPC.
//! The UI decides when focused, actually Playing capture warrants holding it;
//! failure to acquire it is reported without making video capture depend on logind.

use std::os::fd::OwnedFd;

use gtk4::{gio, glib, prelude::UnixFDListExtManual};

/// Hold logind's lock until this value is dropped and its descriptor closes.
pub struct SleepInhibitor {
    _fd: OwnedFd,
}

impl SleepInhibitor {
    pub fn acquire() -> Result<Self, String> {
        let connection = gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE)
            .map_err(|err| format!("System bus unavailable: {err}"))?;
        let (reply, fds) = connection
            .call_with_unix_fd_list_sync(
                Some("org.freedesktop.login1"),
                "/org/freedesktop/login1",
                "org.freedesktop.login1.Manager",
                "Inhibit",
                Some(&glib::Variant::from((
                    "idle:sleep",
                    "Pocket 3 KVM Viewer",
                    "Focused live KVM capture",
                    "block",
                ))),
                None,
                gio::DBusCallFlags::NONE,
                3000,
                gio::UnixFDList::NONE,
                gio::Cancellable::NONE,
            )
            .map_err(|err| format!("logind denied inhibition: {err}"))?;
        let (glib::variant::Handle(index),) = reply
            .get::<(glib::variant::Handle,)>()
            .ok_or("logind returned an invalid inhibitor handle")?;
        let fd = fds
            .ok_or("logind returned no inhibitor descriptors")?
            .get(index)
            .map_err(|err| format!("Cannot receive inhibitor descriptor: {err}"))?;
        Ok(Self { _fd: fd })
    }
}

#[cfg(test)]
mod tests {
    use super::SleepInhibitor;

    #[test]
    #[ignore = "benchmark: requires systemd-logind; no display or capture"]
    fn benchmark_inhibitor_transition_cost() {
        let mut times = Vec::new();
        for _ in 0..20 {
            let began = std::time::Instant::now();
            let lock = SleepInhibitor::acquire().expect("real logind inhibitor");
            times.push(began.elapsed().as_micros());
            drop(lock);
        }
        times.sort_unstable();
        let middle = times.len() / 2;
        let median = (times[middle - 1] + times[middle]) as f64 / 2.0;
        eprintln!(
            "BENCH_INHIBITOR count={} median_us={median:.1} max_us={} total_us={}",
            times.len(),
            times.last().unwrap(),
            times.iter().sum::<u128>()
        );
    }

    #[test]
    #[ignore = "needs a systemd-logind system bus"]
    fn focused_capture_holds_and_releases_a_logind_lock() {
        let lock = SleepInhibitor::acquire().expect("logind must issue an inhibitor FD");
        let inhibitors = std::process::Command::new("systemd-inhibit")
            .arg("--list")
            .output()
            .expect("systemd-inhibit must be installed");
        assert!(String::from_utf8_lossy(&inhibitors.stdout).contains("Pocket 3 KVM Viewer"));
        drop(lock);
        let inhibitors = std::process::Command::new("systemd-inhibit")
            .arg("--list")
            .output()
            .expect("systemd-inhibit must be installed");
        assert!(!String::from_utf8_lossy(&inhibitors.stdout).contains("Pocket 3 KVM Viewer"));
    }
}
