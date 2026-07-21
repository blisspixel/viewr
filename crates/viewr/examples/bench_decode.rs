//! Decode benchmark. Times how long `DecodedImage::load` takes for every image
//! in a corpus directory, reporting median milliseconds and megapixels/second.
//!
//! Dependency-free on purpose (no criterion): plain repeated timing with a
//! median, which is enough to catch regressions and report real numbers.
//!
//! Usage: `cargo run --release --example bench_decode -- [corpus_dir]`.

use std::path::Path;
use std::time::Instant;

use viewr::decode::DecodedImage;

const ITERATIONS: u32 = 5;

fn median(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    samples[samples.len() / 2]
}

fn main() -> anyhow::Result<()> {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "corpus".to_string());
    let mut entries: Vec<_> = std::fs::read_dir(Path::new(&dir))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    entries.sort();

    println!(
        "{:<28} {:>10} {:>12} {:>12}",
        "file", "pixels", "median ms", "MP/s"
    );
    println!("{}", "-".repeat(64));

    for path in entries {
        let mut times = Vec::with_capacity(ITERATIONS as usize);
        let mut pixels = 0u64;
        for _ in 0..ITERATIONS {
            let start = Instant::now();
            let img = match DecodedImage::load(&path) {
                Ok(img) => img,
                Err(e) => {
                    eprintln!("skip {}: {e}", path.display());
                    break;
                }
            };
            times.push(start.elapsed().as_secs_f64() * 1000.0);
            pixels = u64::from(img.width) * u64::from(img.height);
        }
        if times.len() as u32 == ITERATIONS {
            let ms = median(times);
            let mps = (pixels as f64 / 1_000_000.0) / (ms / 1000.0);
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            println!("{name:<28} {pixels:>10} {ms:>12.2} {mps:>12.1}");
        }
    }
    Ok(())
}
