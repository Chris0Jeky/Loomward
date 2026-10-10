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

    fn phase(engine: &Engine, live: bool, seconds: u64) -> Value {
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
        let costs_before = engine.telemetry_costs().unwrap();
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
        for second in 1..=seconds {
            thread::sleep(
                (started + Duration::from_secs(second)).saturating_duration_since(Instant::now()),
            );
            let current = usage();
            private_max = private_max.max(current.private_commit_bytes);
            events += engine.telemetry_events().unwrap().len();
            if live {
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
                if second % 30 == 0 && second < seconds {
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
                    "{}: {second}/{seconds} seconds",
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
        assert!(cpu >= 0.0 && main_cpu >= 0.0 && elapsed >= seconds as f64);
        let costs = engine.telemetry_costs().unwrap();
        json!({
            "sampling_count": costs.samples - costs_before.samples,
            "phase_wall_costs": costs,
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
        let engine = Engine::open(EngineConfig::new(
            "unused-bench-state".into(),
            DatasetClass::Personal,
        ))
        .unwrap();
        let seconds = std::env::args()
            .nth(2)
            .map(|v| v.parse::<u64>().unwrap())
            .unwrap_or(600);
        assert!(seconds > 0);
        let idle = phase(&engine, false, seconds);
        let live = phase(&engine, true, seconds);
        let overhead = live["engine_percent_one_core"].as_f64().unwrap()
            - idle["engine_percent_one_core"].as_f64().unwrap();
        let idle_cpu = idle["engine_percent_one_core"].as_f64().unwrap();
        let idle_commit: u64 = idle["private_commit_true_high_water_bytes"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let result = json!({
            "schema_version":3, "phase_seconds":seconds, "full_duration":seconds>=600, "platform":"Windows 11", "profile":"release", "source_base":option_env!("LOOMWARD_BENCH_BASE").unwrap_or("unrecorded"),
            "scope":"read-only personal observations; persisted evidence is aggregates only",
            "method":"GetProcessTimes sums all engine-process threads; GetThreadTimes measures the harness main thread. Non-main CPU is reported separately without attributing possible Windows library helper threads to the sampler. Same engine: phase_seconds idle then phase_seconds leased, all channels leased at 1000 ms, process enumeration at most every 15 s with retained row timestamps, current system and own counters every tick, renewal every 30 s, bounded events drained every second.",
            "idle":idle, "leased":live,
            "P12": {"target_percent_one_core_above_idle":1.0,"overhead_percent_one_core":overhead,"pass":overhead<=1.0,"no_lease_sampling":{"samples":idle["sampling_count"],"events":idle["events_drained"],"pass":idle["sampling_count"]==0 && idle["events_drained"]==0}},
            "P13": {"target_percent_one_core":0.5,"target_private_commit_bytes":(80 * 1024 * 1024).to_string(),"idle_percent_one_core":idle_cpu,"idle_peak_private_commit_bytes":idle_commit.to_string(),"pass":idle_cpu<=0.5 && idle_commit<=80 * 1024 * 1024},
            "limitations":["Uncontrolled concurrent host workloads and cache conditions.","Benchmark activates telemetry only; catalogue, scan, learning and teacher workloads are not exercised.","A single phase per condition, no p95 or repeatability claim. Phase costs are elapsed wall time, not CPU attribution.","Per-process GPU memory is unsupported; adapter capacity remains unknown when no native observation exists."]
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
