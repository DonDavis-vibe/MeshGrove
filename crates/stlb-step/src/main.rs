//! `stlb-step-mesher INPUT.step OUTPUT.stl [relative-deflection]`
//!
//! Runs as its own process so that a crash inside OpenCascade on a malformed file only
//! loses that one preview. Exit codes: 1 usage, 2 unreadable STEP, 3 no shape, 4 empty shape,
//! 5 meshing failed, 6 cannot write STL, 7/8 OpenCascade exception, 10 built without OpenCascade.

use std::ffi::{CString, c_char, c_int};
use std::process::ExitCode;

#[cfg(not(no_occt))]
unsafe extern "C" {
    fn stlb_step_to_stl(input: *const c_char, output: *const c_char, relative_deflection: f64) -> c_int;
    fn stlb_write_test_cube(output: *const c_char) -> c_int;
}

/// Default chordal tolerance: 0.08 % of the part's bounding-box diagonal.
const DEFAULT_DEFLECTION: f64 = 0.0008;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.as_slice() {
        [flag, out] if flag == "--write-test-cube" => write_test_cube(out),
        [input, output] => convert(input, output, DEFAULT_DEFLECTION),
        [input, output, deflection] => match deflection.parse() {
            Ok(d) => convert(input, output, d),
            Err(_) => 1,
        },
        _ => {
            eprintln!("usage: stlb-step-mesher INPUT.step OUTPUT.stl [relative-deflection]");
            1
        }
    };
    ExitCode::from(code as u8)
}

#[cfg(not(no_occt))]
fn convert(input: &str, output: &str, deflection: f64) -> c_int {
    let (Ok(input), Ok(output)) = (CString::new(input), CString::new(output)) else { return 1 };
    // SAFETY: both pointers are valid NUL-terminated strings for the duration of the call.
    unsafe { stlb_step_to_stl(input.as_ptr(), output.as_ptr(), deflection) }
}

#[cfg(not(no_occt))]
fn write_test_cube(output: &str) -> c_int {
    let Ok(output) = CString::new(output) else { return 1 };
    // SAFETY: valid NUL-terminated string for the duration of the call.
    unsafe { stlb_write_test_cube(output.as_ptr()) }
}

#[cfg(no_occt)]
fn convert(_: &str, _: &str, _: f64) -> c_int {
    eprintln!("stlb-step-mesher was built without OpenCascade");
    10
}

#[cfg(no_occt)]
fn write_test_cube(_: &str) -> c_int {
    10
}
