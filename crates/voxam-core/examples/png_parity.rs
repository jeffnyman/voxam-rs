//! The ported side of the PNG parity sweep: walk one Blorb's Pict
//! resources in index order and, for every PNG aboard, decode and
//! re-encode it -- the Version 6 wire's own transform -- printing
//! the encoded bytes as hexadecimal. The lines must match the
//! reference oracle's byte for byte.

use voxam_core::blorb::Blorb;
use voxam_core::png::{SIGNATURE, decode, encoded};

fn main() -> std::process::ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: png_parity <blorb-file>");
        return std::process::ExitCode::from(2);
    };

    let data = match std::fs::read(&path) {
        Ok(data) => data,
        Err(error) => {
            eprintln!("png_parity: {error}");
            return std::process::ExitCode::from(2);
        }
    };

    let blorb = match Blorb::parse(&data) {
        Ok(blorb) => blorb,
        Err(error) => {
            eprintln!("png_parity: {error}");
            return std::process::ExitCode::from(2);
        }
    };

    let mut out = String::new();

    for resource in &blorb.resources {
        if resource.usage != *b"Pict" {
            continue;
        }

        let payload = &resource.chunk.payload;

        if !payload.starts_with(&SIGNATURE) {
            out.push_str(&format!("pict {} skipped\n", resource.number));
            continue;
        }

        match decode(payload, None) {
            Ok(picture) => {
                out.push_str(&format!("pict {} ", resource.number));

                for byte in encoded(&picture) {
                    out.push_str(&format!("{byte:02x}"));
                }

                out.push('\n');
            }
            Err(error) => {
                out.push_str(&format!("pict {} refused: {error}\n", resource.number));
            }
        }
    }

    print!("{out}");

    std::process::ExitCode::SUCCESS
}
