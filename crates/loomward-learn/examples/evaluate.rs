//! Bounded synthetic smoke; no teacher execution, personal metadata or model promotion.
use loomward_learn::evaluation::{evaluate, fit_isotonic, split_manifest};
use serde_json::{json, Value};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let output = args.next();
    if args.next().is_some() {
        return Err(
            "Usage: cargo run -p loomward-learn --example evaluate -- [output.json]".into(),
        );
    }
    let corpus: Value =
        serde_json::from_str(include_str!("../../../fixtures/evaluation/synthetic.json"))?;
    let manifest = split_manifest(&corpus, 10, 20)?;
    let receipt = json!({"scope": loomward_learn::evaluation::SCOPE,
        "report": evaluate(&corpus, &manifest, 200, 25)?,
        "isotonic_candidate": fit_isotonic(&corpus, &manifest, "calibration")?});
    let text = serde_json::to_string_pretty(&receipt)? + "\n";
    if let Some(output) = output {
        std::fs::write(output, text)?;
    } else {
        print!("{text}");
    }
    Ok(())
}
