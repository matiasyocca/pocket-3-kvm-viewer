use std::os::fd::OwnedFd;

use gtk4::{gio, glib, prelude::UnixFDListExtManual};

// logind's lock exists as long as this descriptor stays open; Drop closes it.
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
