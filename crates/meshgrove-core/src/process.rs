//! Starting our helper programs the same way on every system.

use std::ffi::OsStr;
use std::process::Command;

/// A command for a background helper (PDF renderer, STEP mesher). On Windows it must not open
/// a console window next to the app.
pub fn helper(program: impl AsRef<OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// File name of one of our executables on this system (`name.exe` on Windows).
pub fn exe_name(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}
