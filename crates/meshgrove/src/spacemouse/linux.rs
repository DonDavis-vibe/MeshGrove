//! 3D mice through `/dev/hidraw*`: no library, so no libudev to install. A device is a 3D mouse
//! when its report descriptor in sysfs says "multi-axis controller".

use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use meshgrove_core::spacemouse::is_multi_axis_controller;

use super::{Device, Found};

/// `O_NONBLOCK`: reading never waits, so the thread can notice that the app has gone.
const NON_BLOCKING: i32 = 0o4000;
/// How long to wait between looks for a report (about 200 per second).
const POLL: Duration = Duration::from_millis(5);

struct Hidraw(File);

impl Device for Hidraw {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        match self.0.read(buf) {
            Ok(0) => Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => Ok(Some(n)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(POLL);
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }
}

/// The 3D mice among the hidraw devices of a sysfs folder (`/sys/class/hidraw`): (device node
/// name such as `hidraw3`, product name).
fn list(sysfs: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut entries: Vec<PathBuf> =
        std::fs::read_dir(sysfs).into_iter().flatten().flatten().map(|e| e.path()).collect();
    entries.sort();
    for entry in entries {
        let Some(node) = entry.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
        let Ok(descriptor) = std::fs::read(entry.join("device/report_descriptor")) else { continue };
        if is_multi_axis_controller(&descriptor) {
            out.push((node, product_name(&entry.join("device/uevent"))));
        }
    }
    out
}

/// `HID_NAME=3Dconnexion SpaceNavigator` from a uevent file.
fn product_name(uevent: &Path) -> String {
    std::fs::read_to_string(uevent)
        .ok()
        .and_then(|text| text.lines().find_map(|l| l.strip_prefix("HID_NAME=").map(str::to_owned)))
        .unwrap_or_else(|| "3D mouse".to_owned())
}

pub fn find() -> Found {
    let mut denied = None;
    for (node, name) in list(Path::new("/sys/class/hidraw")) {
        let path = Path::new("/dev").join(&node);
        match OpenOptions::new().read(true).custom_flags(NON_BLOCKING).open(&path) {
            Ok(file) => return Found::Device { name, device: Box::new(Hidraw(file)) },
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => denied = Some(name),
            Err(_) => {}
        }
    }
    denied.map_or(Found::None, Found::NoAccess)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_only_the_multi_axis_controllers_of_sysfs() {
        let dir = tempfile::tempdir().unwrap();
        let device = |node: &str, descriptor: &[u8], name: &str| {
            let d = dir.path().join(node).join("device");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("report_descriptor"), descriptor).unwrap();
            std::fs::write(
                d.join("uevent"),
                format!("DRIVER=hid-generic\nHID_NAME={name}\nHID_ID=0003:0000256F:0000C635\n"),
            )
            .unwrap();
        };
        device("hidraw0", &[0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0xC0], "Keyboard");
        device("hidraw1", &[0x05, 0x01, 0x09, 0x08, 0xA1, 0x01, 0xC0], "3Dconnexion SpaceMouse Compact");
        std::fs::create_dir_all(dir.path().join("hidraw2")).unwrap(); // no descriptor
        assert_eq!(list(dir.path()), [("hidraw1".to_owned(), "3Dconnexion SpaceMouse Compact".to_owned())]);
        assert!(list(&dir.path().join("missing")).is_empty());
    }
}
