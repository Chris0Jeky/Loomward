use loomward_telemetry::Sampler;
use std::{
    io::{self, Write},
    time::{Duration, Instant},
};

const USAGE: &str = "usage: loomward-telemetry snapshot --json | watch --interval-ms 1000 --count N --json (interval 100..60000 ms; count 1..10000)";

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (interval_ms, count) = match args.first().map(String::as_str) {
        Some("snapshot") if args == ["snapshot", "--json"] => (1000, 1),
        Some("watch") => {
            let (mut interval, mut count, mut json) = (None, None, false);
            let mut options = args[1..].iter();
            while let Some(option) = options.next() {
                match option.as_str() {
                    "--json" if !json => json = true,
                    "--interval-ms" if interval.is_none() => {
                        interval = Some(
                            options
                                .next()
                                .ok_or(USAGE)?
                                .parse::<u64>()
                                .map_err(|_| USAGE)?,
                        )
                    }
                    "--count" if count.is_none() => {
                        count = Some(
                            options
                                .next()
                                .ok_or(USAGE)?
                                .parse::<u64>()
                                .map_err(|_| USAGE)?,
                        )
                    }
                    _ => return Err(USAGE.into()),
                }
            }
            let interval = interval.unwrap_or(1000);
            let count = count.ok_or(USAGE)?;
            if !json || !(100..=60000).contains(&interval) || !(1..=10000).contains(&count) {
                return Err(USAGE.into());
            }
            (interval, count)
        }
        _ => return Err(USAGE.into()),
    };
    let mut sampler = Sampler::default();
    let stdout = io::stdout();
    let mut output = stdout.lock();
    for index in 0..count {
        let started = Instant::now();
        let snapshot = sampler.sample(200);
        serde_json::to_writer(&mut output, &snapshot).map_err(|e| e.to_string())?;
        writeln!(output).map_err(|e| e.to_string())?;
        output.flush().map_err(|e| e.to_string())?;
        if index + 1 < count {
            std::thread::sleep(
                Duration::from_millis(interval_ms).saturating_sub(started.elapsed()),
            );
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(2);
    }
}
