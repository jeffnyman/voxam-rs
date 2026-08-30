//! The ported side of the gallery parity sweep: hang one Blorb's
//! gallery and print its census -- release, Reso window and
//! scaling fractions, adaptive numbers, baked records, and every
//! picture's size beside its 640-by-400 scaling ratio. The lines
//! must match the reference oracle's byte for byte.

use voxam_core::blorb::Blorb;
use voxam_core::gallery::Ratio;

fn spelled(ratio: Option<Ratio>) -> String {
    match ratio {
        None => "-".to_string(),
        Some(held) => format!("{}/{}", held.numerator(), held.denominator()),
    }
}

fn main() -> std::process::ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: gallery_parity <blorb-file>");
        return std::process::ExitCode::from(2);
    };

    let data = match std::fs::read(&path) {
        Ok(data) => data,
        Err(error) => {
            eprintln!("gallery_parity: {error}");
            return std::process::ExitCode::from(2);
        }
    };

    let blorb = match Blorb::parse(&data) {
        Ok(blorb) => blorb,
        Err(error) => {
            eprintln!("gallery_parity: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let gallery = match blorb.gallery() {
        Ok(gallery) => gallery,
        Err(error) => {
            eprintln!("gallery_parity: {error}");
            return std::process::ExitCode::from(2);
        }
    };

    println!("release {}", gallery.release);

    if let Some(held) = &blorb.resolution {
        println!("window {}x{}", held.width, held.height);

        for (number, scaling) in &held.scalings {
            println!(
                "scaling {number} {} {} {}",
                spelled(Some(scaling.standard)),
                spelled(scaling.minimum),
                spelled(scaling.maximum)
            );
        }
    }

    if !blorb.adaptive.is_empty() {
        let mut numbers: Vec<u32> = blorb.adaptive.iter().copied().collect();

        numbers.sort_unstable();

        let told: Vec<String> = numbers.iter().map(u32::to_string).collect();

        println!("adaptive {}", told.join(" "));
    }

    let mut baked: Vec<((u32, u32), u32)> = blorb
        .baked
        .iter()
        .map(|(&pair, &replacement)| (pair, replacement))
        .collect();

    baked.sort_unstable();

    for ((scene, adaptive), replacement) in baked {
        println!("baked {scene} {adaptive} {replacement}");
    }

    for number in gallery.numbers().collect::<Vec<_>>() {
        let (height, width) = match gallery.size(number) {
            Ok(Some(size)) => size,
            Ok(None) => continue,
            Err(error) => {
                eprintln!("gallery_parity: {error}");
                return std::process::ExitCode::from(2);
            }
        };
        let ratio = gallery.scale(number, 640, 400);

        println!("pict {number} {height}x{width} {}", spelled(Some(ratio)));
    }

    std::process::ExitCode::SUCCESS
}
