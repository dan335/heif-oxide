//! Decode a HEIF/HEIC file and write the result as a binary PPM (P6) —
//! dependency-free visual smoke test:
//!
//! ```text
//! cargo run --release --example decode -- photo.heic out.ppm
//! ```

use std::io::Write;

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("usage: decode <in.heic> [out.ppm]");
    let output = args.next();

    let start = std::time::Instant::now();
    let image = match heif_oxide::decode_file(&input) {
        Ok(image) => image,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "{input}: {}x{}, {} channels, {}-bit, nclx {:?}, decoded in {:.0?}",
        image.width,
        image.height,
        image.channels(),
        image.bit_depth(),
        image.color.nclx,
        start.elapsed()
    );

    if let Some(output) = output {
        // PPM is RGB-only; flatten alpha if present.
        let rgba = image.to_rgba8();
        let mut ppm = Vec::with_capacity(rgba.len() / 4 * 3 + 64);
        write!(ppm, "P6\n{} {}\n255\n", image.width, image.height).unwrap();
        for px in rgba.as_chunks::<4>().0 {
            ppm.extend_from_slice(&px[..3]);
        }
        std::fs::write(&output, ppm).expect("write output");
        println!("wrote {output}");
    }
}
