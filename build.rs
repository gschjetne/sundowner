//! Compresses the fonts of the `silk` tier into `OUT_DIR`, so the binary
//! carries them zlib-compressed and inflates each one only when a document
//! needs it. Uses sundowner's own DEFLATE encoder; the base fonts are
//! embedded as-is.

use std::path::Path;

#[allow(dead_code)]
#[path = "src/flate.rs"]
mod flate;

/// The fonts of the `silk` tier, in `fonts/silk/`.
const SILK: [&str; 10] = [
    "FrankRuhlLibre-Regular.ttf",
    "FrankRuhlLibre-Bold.ttf",
    "NotoSerifHebrew-Regular.ttf",
    "NotoSerifHebrew-Bold.ttf",
    "NotoSerifArmenian-Regular.ttf",
    "NotoSerifArmenian-Bold.ttf",
    "NotoSerifSC-Regular.ttf",
    "NotoSerifSC-Bold.ttf",
    "GowunBatang-Regular.ttf",
    "GowunBatang-Bold.ttf",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/flate.rs");
    if std::env::var_os("CARGO_FEATURE_SILK").is_none() {
        return;
    }
    let out = std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR");
    let out = Path::new(&out);
    std::thread::scope(|s| {
        for name in SILK {
            s.spawn(move || {
                let src = Path::new("fonts/silk").join(name);
                println!("cargo:rerun-if-changed={}", src.display());
                let data = std::fs::read(&src).unwrap_or_else(|e| panic!("{}: {e}", src.display()));
                let z = flate::zlib_compress(&data);
                std::fs::write(out.join(format!("{name}.z")), z).expect("OUT_DIR is writable");
            });
        }
    });
}
