//! CPU-only terminal parsing and viewport capture; no GPUI, GPU, IPC, or SSH.
//! Run through scripts/with-ghostty.mjs with cargo run --release.
use newport_terminal::Engine;
use std::{hint::black_box, time::Instant};

fn percentile(sorted: &[f64], percent: usize) -> f64 {
    sorted[(sorted.len() * percent).div_ceil(100).saturating_sub(1)]
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        return Err("use --release for recorded performance measurements".into());
    }
    const BYTES: usize = 4 * 1024 * 1024;
    const CHUNK: usize = 4096;
    println!("case,run,input_bytes,batches,elapsed_ms,mib_per_s,p50_ms,p95_ms,p99_ms,max_ms");
    for (name, pattern) in [
        (
            "ascii",
            "2026-10-13 INFO request=1234 status=200 elapsed=12ms path=/health\r\n",
        ),
        (
            "ansi",
            "\x1b[32mINFO\x1b[0m request=1234 \x1b[1;31mERROR\x1b[0m detail=retry\r\n",
        ),
        ("unicode", "日本語 中文 e\u{301} 🪉 🫩 👩‍💻 response=完了\r\n"),
    ] {
        // Whole patterns preserve a complete final UTF-8 sequence. Individual
        // transport-sized chunks deliberately may split UTF-8 and VT sequences.
        let payload = pattern.repeat(BYTES.div_ceil(pattern.len()));
        for run in 1..=3 {
            let mut engine = Engine::new(160, 48)?;
            let warmup = pattern.repeat(128);
            engine.advance(warmup.as_bytes())?;
            black_box(&*engine.frame()?);
            let mut samples = Vec::with_capacity(payload.len().div_ceil(CHUNK));
            let started = Instant::now();
            for bytes in payload.as_bytes().chunks(CHUNK) {
                let batch = Instant::now();
                black_box(engine.advance(bytes)?);
                black_box(&*engine.frame()?);
                samples.push(batch.elapsed().as_secs_f64() * 1000.);
            }
            let seconds = started.elapsed().as_secs_f64();
            // Ensure the measured stream leaves a working parser and viewport.
            // This is outside the measured interval, not a throughput shortcut.
            engine.advance(b"\r\nNEWPORT_BENCHMARK_COMPLETE")?;
            assert!(
                engine
                    .frame()?
                    .visible_lines()
                    .iter()
                    .any(|line| { line.contains("NEWPORT_BENCHMARK_COMPLETE") }),
                "terminal stopped accepting output"
            );
            samples.sort_unstable_by(f64::total_cmp);
            println!(
                "{name},{run},{},{},{:.3},{:.3},{:.4},{:.4},{:.4},{:.4}",
                payload.len(),
                samples.len(),
                seconds * 1000.,
                payload.len() as f64 / (1024. * 1024.) / seconds,
                percentile(&samples, 50),
                percentile(&samples, 95),
                percentile(&samples, 99),
                samples.last().unwrap(),
            );
        }
    }
    Ok(())
}
