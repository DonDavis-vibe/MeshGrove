//! 3D mice on Windows and macOS through the `hidapi` crate: HID devices with the usage
//! "multi-axis controller" (generic desktop page, usage 8), which is what 3Dconnexion devices and
//! DIY builds that copy them report.

use std::io;

use super::{Device, Found};

/// How long one read waits for a report.
const WAIT_MS: i32 = 20;

struct Hid(hidapi::HidDevice);

impl Device for Hid {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        match self.0.read_timeout(buf, WAIT_MS) {
            Ok(0) => Ok(None),
            Ok(n) => Ok(Some(n)),
            Err(e) => Err(io::Error::other(e.to_string())),
        }
    }
}

pub fn find() -> Found {
    let Ok(api) = hidapi::HidApi::new() else { return Found::None };
    let mut denied = None;
    for info in api.device_list().filter(|d| d.usage_page() == 0x01 && d.usage() == 0x08) {
        let name = info.product_string().unwrap_or("3D mouse").to_owned();
        match api.open_path(info.path()) {
            Ok(device) => return Found::Device { name, device: Box::new(Hid(device)) },
            Err(_) => denied = Some(name),
        }
    }
    denied.map_or(Found::None, Found::NoAccess)
}
