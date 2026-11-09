//! Headless simulation runner for debugging.
//!
//! cargo run -p sim --release --bin simrun -- [--seed N] [--days D] [--every H]
//!     [--no-orders] [--map Z] [--save FILE] [--load FILE] [--events]

use std::time::Instant;

use sim::{Sim, debug, resources::{SimMeta, TICKS_PER_HOUR}, scenario, worldgen::WorldGenParams};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let has = |k: &str| args.iter().any(|a| a == k);
    let seed: u64 = get("--seed").and_then(|s| s.parse().ok()).unwrap_or(42);
    let days: f32 = get("--days").and_then(|s| s.parse().ok()).unwrap_or(2.0);
    let every_h: u64 = get("--every").and_then(|s| s.parse().ok()).unwrap_or(6);
    let defs_dir = get("--defs").unwrap_or_else(|| "assets/defs".into());

    let defs = Sim::load_defs(std::path::Path::new(&defs_dir)).unwrap_or_else(|e| {
        eprintln!("def error: {e}");
        std::process::exit(1)
    });
    let t0 = Instant::now();
    let mut sim = Sim::new(defs, seed, &WorldGenParams::default());
    println!("worldgen: {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);
    if let Some(f) = get("--load") {
        let bytes = std::fs::read(&f).expect("read save");
        sim.load_bytes(&bytes).expect("load");
        println!("loaded {f}");
    } else if !has("--no-orders") {
        scenario::basic_orders(&mut sim);
    }
    if let Some(z) = get("--map").and_then(|s| s.parse::<i32>().ok()) {
        let start = sim.world.res::<SimMeta>().start;
        println!("{}", debug::ascii_level(&sim.world, z, start, 30));
    }

    let total = (days * 24.0 * TICKS_PER_HOUR as f32) as u64;
    let every = every_h * TICKS_PER_HOUR;
    let t0 = Instant::now();
    let mut done = 0;
    let mut seen = 0u64;
    while done < total {
        let n = every.min(total - done);
        for i in 0..n {
            sim.tick();
            if has("--events") && (done + i) % 300 == 0 {
                let log = sim.world.res::<sim::resources::GameLog>();
                for e in log.entries.iter().filter(|e| e.tick > seen) {
                    println!("  ev: d{} {}", e.tick / sim::resources::TICKS_PER_DAY + 1, e.text);
                }
                seen = log.entries.last().map_or(seen, |e| e.tick);
            }
            if (done + i) % 600 == 0 {
                scenario::ensure_bills(&mut sim);
                if !has("--no-orders") {
                    scenario::periodic_orders(&mut sim);
                }
            }
        }
        done += n;
        println!("{}", debug::colony_report(&sim.world));
        for l in debug::recent_log(&sim.world, 6) {
            println!("  log: {l}");
        }
    }
    let el = t0.elapsed().as_secs_f64();
    println!("ran {total} ticks in {el:.2}s ({:.0} ticks/s, {:.3} ms/tick)", total as f64 / el, el * 1000.0 / total as f64);
    if let Some(z) = get("--map-end").and_then(|s| s.parse::<i32>().ok()) {
        let start = sim.world.res::<SimMeta>().start;
        println!("{}", debug::ascii_level(&sim.world, z, start, 30));
    }
    println!("state hash: {:016x}", sim.state_hash());
    if let Some(f) = get("--save") {
        std::fs::write(&f, sim.save_bytes().expect("save")).expect("write");
        println!("saved {f}");
    }
}
