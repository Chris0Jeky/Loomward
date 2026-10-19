//! Issue #700: time validation and a complete service open on an existing synthetic store.
//! Build the fixture with loomward-catalog's quick_check_measure example first.
//! cargo run --release -p loomward-service --example startup_measure -- <dir> [runs] [label]
use loomward_catalog::Catalog;
use loomward_protocol::DatasetClass;
use loomward_service::{Config, Service};
use serde_json::json;
use std::{path::PathBuf, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(args.get(1).ok_or("expected synthetic store directory")?);
    let runs: usize = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(3);
    let label = args.get(3).map(String::as_str).unwrap_or("run");
    for run in 1..=runs {
        let start = Instant::now();
        Catalog::validate_stores(&dir, "synthetic")?;
        let validate_ms = start.elapsed().as_secs_f64() * 1000.0;
        let start = Instant::now();
        let service = Service::open(Config {
            state_dir: dir.clone(),
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            allow_identity_unverified: false,
            grant_roots: vec![],
        })?;
        let open_ms = start.elapsed().as_secs_f64() * 1000.0;
        drop(service);
        println!(
            "{}",
            json!({"label": label, "run": run,
                "validate_stores_ms": validate_ms, "service_open_ms": open_ms})
        );
    }
    Ok(())
}
