//! Loads model files and prints what the browser would show — handy for checking real libraries.
//!
//!     cargo run --release -p stlb-core --example probe -- file.3mf other.stl …

use std::time::Instant;

use stlb_core::loaders::{self, threemf};
use stlb_core::{Kind, dedupe};

fn main() {
    for path in std::env::args().skip(1) {
        let kind = Kind::from_file_name(&path);
        let short = path.rsplit('/').next().unwrap_or(&path);
        let Ok(bytes) = std::fs::read(&path) else {
            println!("READ-ERROR {short}");
            continue;
        };
        let t = Instant::now();
        let result = loaders::load_mesh(kind, &bytes);
        let ms = t.elapsed().as_millis();
        let extra = if kind == Kind::ThreeMf {
            let thumb = threemf::embedded_thumbnail(&bytes, 384).map(|i| format!("{}x{}", i.width(), i.height()));
            let plates = threemf::plate_images(&bytes).len();
            let app = threemf::parse(&bytes).ok().and_then(|m| m.application).unwrap_or_default();
            format!("thumb={} plates={plates} app={app}", thumb.unwrap_or_else(|| "-".into()))
        } else {
            String::new()
        };
        match result {
            Ok(mesh) => {
                let s = mesh.bounds().map(|b| b.size()).unwrap_or_default();
                let sig = dedupe::geo_signature(&mesh).unwrap_or_default();
                println!(
                    "OK {:>9} tris {:6.1}x{:6.1}x{:6.1} mm {ms:>5} ms {extra} sig={} :: {short}",
                    mesh.triangle_count(),
                    s.x,
                    s.y,
                    s.z,
                    &sig[..sig.len().min(18)]
                );
            }
            Err(e) => println!("FAIL {e} {extra} :: {short}"),
        }
    }
}
