//! Reading 3D mice (SpaceMouse and look-alikes) in a thread of their own. The newest state sits
//! in shared memory, the thread wakes the interface when something moved. Linux reads the
//! `/dev/hidraw*` devices itself, Windows and macOS go through the `hidapi` crate.

use std::io;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use meshgrove_core::spacemouse::{Report, parse_report};

#[cfg(any(windows, target_os = "macos"))]
mod hid;
#[cfg(target_os = "linux")]
mod linux;

/// What the reader knows about the device.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Status {
    /// Nothing that looks like a 3D mouse is plugged in.
    #[default]
    NoDevice,
    /// Reading from this device (its name).
    Connected(String),
    /// A 3D mouse is there, but this user may not read it (Linux: needs a udev rule).
    NoAccess(String),
}

/// The newest state of the device, as the interface sees it.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub status: Status,
    /// The raw axes (X, Y, Z, rotation X, Y, Z) in −1…1.
    pub raw: [f32; 6],
    /// Buttons pressed now, one bit each.
    pub buttons: u32,
}

#[derive(Default)]
struct Shared {
    snapshot: Snapshot,
    /// Buttons that went down since the interface last asked, in order.
    presses: Vec<u8>,
}

/// A running reader; dropping it stops the thread.
pub struct SpaceMouse {
    shared: Arc<Mutex<Shared>>,
}

/// An open device: reads one report at a time.
trait Device: Send {
    /// The next report, or `None` after a short wait without one. An error means the device
    /// is gone.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>>;
}

enum Found {
    Device { name: String, device: Box<dyn Device> },
    NoAccess(String),
    None,
}

impl SpaceMouse {
    pub fn start(ctx: egui::Context) -> SpaceMouse {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let weak = Arc::downgrade(&shared);
        let spawned = std::thread::Builder::new().name("spacemouse".into()).spawn(move || run(&weak, &ctx));
        if let Err(e) = spawned {
            log::warn!("3D mouse: no thread: {e}");
        }
        SpaceMouse { shared }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().map(|s| s.snapshot.clone()).unwrap_or_default()
    }

    /// The buttons that were pressed since the last call.
    pub fn take_presses(&self) -> Vec<u8> {
        self.shared.lock().map(|mut s| std::mem::take(&mut s.presses)).unwrap_or_default()
    }
}

/// How often to look for a device while there is none.
const SCAN_EVERY: Duration = Duration::from_millis(1500);

fn run(shared: &Weak<Mutex<Shared>>, ctx: &egui::Context) {
    loop {
        let found = find();
        let Some(strong) = shared.upgrade() else { return };
        let status = match &found {
            Found::Device { name, .. } => Status::Connected(name.clone()),
            Found::NoAccess(name) => Status::NoAccess(name.clone()),
            Found::None => Status::NoDevice,
        };
        let changed = {
            let Ok(mut s) = strong.lock() else { return };
            let changed = s.snapshot.status != status;
            s.snapshot = Snapshot { status, ..Snapshot::default() };
            changed
        };
        drop(strong);
        if changed {
            ctx.request_repaint();
        }
        match found {
            Found::Device { name, device } => {
                log::info!("3D mouse: {name}");
                if !read_loop(shared, ctx, device) {
                    return;
                }
                let Some(strong) = shared.upgrade() else { return };
                if let Ok(mut s) = strong.lock() {
                    s.snapshot = Snapshot::default();
                }
                ctx.request_repaint();
            }
            Found::NoAccess(_) | Found::None => std::thread::sleep(SCAN_EVERY),
        }
    }
}

/// Reads until the device is gone (true) or the app has dropped the reader (false).
fn read_loop(shared: &Weak<Mutex<Shared>>, ctx: &egui::Context, mut device: Box<dyn Device>) -> bool {
    let mut buf = [0u8; 64];
    loop {
        let n = match device.read(&mut buf) {
            Ok(Some(n)) => n,
            Ok(None) => {
                if shared.strong_count() == 0 {
                    return false;
                }
                continue;
            }
            Err(_) => return true,
        };
        let Some(report) = parse_report(&buf[..n]) else { continue };
        let Some(strong) = shared.upgrade() else { return false };
        let Ok(mut s) = strong.lock() else { return false };
        let moved = apply(&mut s, report);
        drop(s);
        if moved {
            ctx.request_repaint();
        }
    }
}

/// Puts a report into the shared state; whether anything the interface cares about changed.
fn apply(shared: &mut Shared, report: Report) -> bool {
    let raw = &mut shared.snapshot.raw;
    match report {
        Report::Translation(t, rotation) => {
            let before = *raw;
            raw[..3].copy_from_slice(&t);
            if let Some(r) = rotation {
                raw[3..].copy_from_slice(&r);
            }
            *raw != before
        }
        Report::Rotation(r) => {
            let before = *raw;
            raw[3..].copy_from_slice(&r);
            *raw != before
        }
        Report::Buttons(bits) => {
            let down = bits & !shared.snapshot.buttons;
            shared.snapshot.buttons = bits;
            for bit in 0..32u8 {
                if down & (1 << bit) != 0 {
                    shared.presses.push(bit);
                }
            }
            down != 0
        }
    }
}

#[cfg(target_os = "linux")]
fn find() -> Found {
    linux::find()
}

#[cfg(any(windows, target_os = "macos"))]
fn find() -> Found {
    hid::find()
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn find() -> Found {
    Found::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_update_the_shared_state_and_button_presses_queue_up() {
        let mut shared = Shared::default();
        assert!(apply(&mut shared, Report::Translation([0.5, 0.0, 0.0], None)));
        assert!(!apply(&mut shared, Report::Translation([0.5, 0.0, 0.0], None)), "no change");
        assert!(apply(&mut shared, Report::Rotation([0.0, 0.0, 1.0])));
        assert_eq!(shared.snapshot.raw, [0.5, 0.0, 0.0, 0.0, 0.0, 1.0]);
        assert!(apply(&mut shared, Report::Translation([0.0; 3], Some([0.25; 3]))));
        assert_eq!(shared.snapshot.raw[3], 0.25);
        assert!(apply(&mut shared, Report::Buttons(0b01)));
        assert!(!apply(&mut shared, Report::Buttons(0b01)), "held, not pressed again");
        assert!(apply(&mut shared, Report::Buttons(0b11)));
        assert!(!apply(&mut shared, Report::Buttons(0b00)), "release is not a press");
        assert_eq!(shared.presses, [0, 1]);
    }
}
