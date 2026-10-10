//! P12/P13: same engine, 10 min idle then 10 min at 1 Hz. Aggregate output only.
#![forbid(unsafe_code)]
//! Run: cargo run --release -p loomward-engine --example telemetry_bench -- evidence/v3/bench/telemetry.json
#[cfg(windows)]
mod windows {
    use loomward_engine::{Engine, EngineConfig};
    use loomward_protocol::*;
    use loomward_telemetry::{own_usage, Observation, OwnUsage};
    use serde_json::{json, Value};
    use std::thread;
    use std::time::{Duration, Instant};

    fn usage() -> OwnUsage {
        match own_usage() {
            Observation::Observed { value } => value,
            _ => panic!("UNVERIFIED: own CPU/memory observation failed"),
        }
    }

    fn phase(engine: &Engine, live: bool) -> Value {
        let request = TelemetrySubscribeRequest {
            subscription_id: None,
            channels: vec![
                TelemetryChannel::System,
                TelemetryChannel::Processes,
                TelemetryChannel::Gpu,
                TelemetryChannel::Disks,
                TelemetryChannel::Engine,
            ]
            .try_into()
            .unwrap(),
            interval_ms: TelemetryInterval::OneSecond,
        };
        let before = usage();
        let started = Instant::now();
        let mut subscription = live.then(|| engine.telemetry_lease(&request).unwrap());
        let mut events = 0usize;
        let mut own_commit_samples = 0;
        let mut previous_seq = 0;
        let snapshot = TelemetrySnapshotRequest {
            channels: request.channels.clone(),
        };
        let mut private_max = before.private_commit_bytes;
        for second in 1..=600 {
            thread::sleep(
                (started + Duration::from_secs(second)).saturating_duration_since(Instant::now()),
            );
            let current = usage();
            private_max = private_max.max(current.private_commit_bytes);
            if live {
                events += engine.telemetry_events().unwrap().len();
                if let Ok(sample) = engine.telemetry_snapshot(&snapshot) {
                    if sample.sample_seq.get() > previous_seq {
                        previous_seq = sample.sample_seq.get();
                        if let Some(own) = sample.engine {
                            if own.private_commit_bytes.is_some() {
                                own_commit_samples += 1;
                            }
                        }
                    }
                }
                if second % 30 == 0 && second < 600 {
                    subscription = Some(
                        engine
                            .telemetry_lease(&TelemetrySubscribeRequest {
                                subscription_id: subscription
                                    .as_ref()
                                    .map(|s| s.subscription_id.clone()),
                                ..request.clone()
                            })
                            .unwrap(),
                    );
                }
            }
            if second % 60 == 0 {
                eprintln!(
                    "{}: {second}/600 seconds",
                    if live { "leased" } else { "idle" }
                );
            }
        }
        if let Some(subscription) = subscription {
            engine
                .telemetry_release(&SubscriptionRefRequest {
                    subscription_id: subscription.subscription_id,
                })
                .unwrap();
        }
        let after = usage();
        let elapsed = started.elapsed().as_secs_f64();
        let cpu = after.cpu_seconds - before.cpu_seconds;
        let main_cpu = after.calling_thread_cpu_seconds - before.calling_thread_cpu_seconds;
        assert!(cpu >= 0.0 && main_cpu >= 0.0 && elapsed >= 600.0);
        json!({
            "duration_s": elapsed, "engine_cpu_s": cpu, "main_thread_cpu_s": main_cpu,
            "non_main_thread_cpu_s": cpu - main_cpu,
            "engine_percent_one_core": cpu / elapsed * 100.0,
            "non_main_thread_percent_one_core": (cpu - main_cpu) / elapsed * 100.0,
            "private_commit_start_bytes": before.private_commit_bytes.to_string(),
            "private_commit_end_bytes": after.private_commit_bytes.to_string(),
            "private_commit_observed_max_bytes": private_max.max(after.private_commit_bytes).to_string(),
            "private_commit_true_high_water_bytes": after.peak_private_commit_bytes.to_string(),
            "events_drained": events, "last_sample_seq": previous_seq,
            "own_commit_samples": own_commit_samples,
        })
    }

    pub fn run() {
        let output = std::env::args()
            .nth(1)
            .expect("aggregate evidence JSON output path required");
        let engine = Engine::open(EngineConfig {
            state_dir: "unused-bench-state".into(),
            dataset_class: DatasetClass::Personal,
        })
        .unwrap();
        let idle = phase(&engine, false);
        let live = phase(&engine, true);
        let overhead = live["engine_percent_one_core"].as_f64().unwrap()
            - idle["engine_percent_one_core"].as_f64().unwrap();
        let idle_cpu = idle["engine_percent_one_core"].as_f64().unwrap();
        let idle_commit: u64 = idle["private_commit_true_high_water_bytes"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let result = json!({
            "schema_version":1, "platform":"Windows 11", "profile":"release", "source_base":option_env!("LOOMWARD_BENCH_BASE").unwrap_or("unrecorded"),
            "scope":"read-only personal observations; persisted evidence is aggregates only",
            "method":"GetProcessTimes sums all engine-process threads; GetThreadTimes measures the harness main thread. Non-main CPU is reported separately without attributing possible Windows library helper threads to the sampler. Same engine: 600 s idle then 600 s leased, all channels at 1000 ms, renewal every 30 s, bounded events drained every second.",
            "idle":idle, "leased":live,
            "P12": {"target_percent_one_core_above_idle":1.0,"overhead_percent_one_core":overhead,"pass":overhead<=1.0,"no_lease_sampling":"none: lease lifecycle tested independently"},
            "P13": {"target_percent_one_core":0.5,"target_private_commit_bytes":(80 * 1024 * 1024).to_string(),"idle_percent_one_core":idle_cpu,"idle_peak_private_commit_bytes":idle_commit.to_string(),"pass":idle_cpu<=0.5 && idle_commit<=80 * 1024 * 1024},
            "limitations":["Uncontrolled concurrent host workloads and cache conditions.","C2 engine with telemetry only: catalogue, scan, learning and teacher pools remain stubs.","A single 10 minute phase per condition, no p95 or repeatability claim.","Per-process GPU memory is unsupported; adapter capacity remains unknown when no native observation exists."]
        });
        if let Some(parent) = std::path::Path::new(&output).parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(
            output,
            serde_json::to_string_pretty(&result).unwrap() + "\n",
        )
        .unwrap();
        println!("{}", json!({"P12":result["P12"],"P13":result["P13"]}));
    }
}

#[cfg(windows)]
fn main() {
    windows::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("UNVERIFIED: P12/P13 require Windows");
    std::process::exit(1);
}
