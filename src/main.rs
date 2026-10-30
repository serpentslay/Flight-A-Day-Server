mod artifact;
mod domain;
mod pipeline;
mod provider;
mod scoring;
mod service;
mod storage;
mod test_data;

use std::{env, path::PathBuf, process::ExitCode};

use artifact::{ArtifactPaths, DisplayProfile, render_fixture};
use pipeline::run_fixture_pipeline;
use provider::{FixtureProvider, FlightProvider};
use storage::{StatusRecord, Storage};

const HELP: &str = "Flight A Day

Usage:
  flight-a-day serve [--once]
  flight-a-day run-now [--date YYYY-MM-DD]
  flight-a-day refresh-test [--date YYYY-MM-DD]
  flight-a-day health
  flight-a-day generate-test-data [--output DIR] [--days N] [--flights-per-day N]
  flight-a-day render-fixture [display options]
  flight-a-day run-fixture [--input FILE] [--date YYYY-MM-DD] [pipeline options]
  flight-a-day status [--date YYYY-MM-DD] [--database FILE]
  flight-a-day show-selection --date YYYY-MM-DD [--database FILE]

Options:
  --input FILE     Candidate fixture (default: fixtures/flights.json)
  --date DATE      Target date; defaults to fixture date for run-fixture
  --database FILE  SQLite database (default: data/flight-a-day.db)
  --output DIR     Artifact directory (default: out)
  --width PX       Native display width (default: 800)
  --height PX      Native display height (default: 480)
  --rotation DEG   Content rotation: 0, 90, 180, or 270 (default: 90)
  --black-bit N    Bit value used for black pixels (default: 0)
  -h, --help       Show this help
";

#[tokio::main]
async fn main() -> ExitCode {
    match run(env::args().skip(1).collect()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Vec<String>) -> Result<(), String> {
    if args.first().is_some_and(|command| {
        matches!(
            command.as_str(),
            "serve" | "run-now" | "refresh-test" | "health"
        )
    }) {
        return service::run(&args).await;
    }
    if args
        .first()
        .is_some_and(|command| command == "generate-test-data")
    {
        return test_data::run(&args[1..]);
    }
    let Some(command) = args.first().map(String::as_str) else {
        print!("{HELP}");
        return Ok(());
    };
    if matches!(command, "-h" | "--help") {
        print!("{HELP}");
        return Ok(());
    }
    if !matches!(
        command,
        "render-fixture" | "run-fixture" | "status" | "show-selection"
    ) {
        return Err(format!("unknown command '{command}'\n\n{HELP}"));
    }

    let mut output = PathBuf::from("out");
    let mut input = PathBuf::from("fixtures/flights.json");
    let mut database = PathBuf::from("data/flight-a-day.db");
    let mut date = None;
    let mut profile = DisplayProfile::default();
    let mut index = 1;

    while index < args.len() {
        let flag = &args[index];
        if matches!(flag.as_str(), "-h" | "--help") {
            print!("{HELP}");
            return Ok(());
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing value for {flag}"))?;

        match flag.as_str() {
            "--input" if command == "run-fixture" => input = PathBuf::from(value),
            "--date" if command != "render-fixture" => date = Some(value.clone()),
            "--database" if command != "render-fixture" => database = PathBuf::from(value),
            "--output" if matches!(command, "render-fixture" | "run-fixture") => {
                output = PathBuf::from(value)
            }
            "--width" if matches!(command, "render-fixture" | "run-fixture") => {
                profile.width = parse_u32(flag, value)?
            }
            "--height" if matches!(command, "render-fixture" | "run-fixture") => {
                profile.height = parse_u32(flag, value)?
            }
            "--rotation" if matches!(command, "render-fixture" | "run-fixture") => {
                profile.rotation_degrees = parse_u16(flag, value)?
            }
            "--black-bit" if matches!(command, "render-fixture" | "run-fixture") => {
                let parsed = parse_u8(flag, value)?;
                if parsed > 1 {
                    return Err("--black-bit must be 0 or 1".into());
                }
                profile.black_bit = parsed;
            }
            _ => return Err(format!("unknown option '{flag}' for {command}")),
        }
        index += 2;
    }

    match command {
        "render-fixture" => {
            let artifacts = render_fixture(&output, profile).map_err(|error| error.to_string())?;
            print_artifacts(&artifacts);
        }
        "run-fixture" => {
            let provider = FixtureProvider::from_path(&input).map_err(|error| error.to_string())?;
            let requested_date = date.unwrap_or_else(|| provider.date().to_owned());
            let storage = Storage::connect(&database)
                .await
                .map_err(|error| error.to_string())?;
            let result =
                run_fixture_pipeline(&storage, &provider, &requested_date, &output, profile)
                    .await?;

            println!(
                "Selected {} from {} {} candidates with score {} (display {})",
                result.selected.flight.callsign,
                result.candidate_count,
                provider.name(),
                result.selected.score.total,
                result.display_number
            );
            println!("{}", result.selected.explanation);
            println!(
                "Artifacts: {}",
                if result.artifacts_reused {
                    "reused after checksum verification"
                } else {
                    "rendered"
                }
            );
            print_artifacts(&result.artifacts);
        }
        "status" => {
            let storage = Storage::connect(&database)
                .await
                .map_err(|error| error.to_string())?;
            match storage
                .status(date.as_deref())
                .await
                .map_err(|error| error.to_string())?
            {
                Some(status) => print_status(&status),
                None => println!("No pipeline runs have been recorded."),
            }
        }
        "show-selection" => {
            let date = date.ok_or_else(|| "show-selection requires --date".to_owned())?;
            let storage = Storage::connect(&database)
                .await
                .map_err(|error| error.to_string())?;
            let status = storage
                .status(Some(&date))
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("no pipeline run exists for {date}"))?;
            let selected = status
                .selected
                .ok_or_else(|| format!("run {date} has no recorded selection"))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&selected).map_err(|error| error.to_string())?
            );
        }
        _ => unreachable!("command was validated above"),
    }
    Ok(())
}

fn print_status(status: &StatusRecord) {
    println!("Date:       {}", status.date);
    println!("Run:        {}", status.status);
    println!("Stage:      {}", status.stage);
    println!("Attempts:   {}", status.attempt_count);
    if let Some(selected) = &status.selected {
        println!("Selected:   {}", selected.callsign);
    }
    if let Some(score) = status.score_total {
        println!("Score:      {score}");
    }
    if let Some(number) = &status.display_number {
        println!("Display:    {number}");
    }
    if let Some(error) = &status.error {
        println!("Error:      {error}");
    }
}

fn print_artifacts(artifacts: &ArtifactPaths) {
    println!("Generated deterministic artifacts:");
    println!("  {}", artifacts.binary.display());
    println!("  {}", artifacts.preview.display());
    println!("  {}", artifacts.manifest.display());
}

fn parse_u32(flag: &str, value: &str) -> Result<u32, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} expects a positive integer, got '{value}'"))
}

fn parse_u16(flag: &str, value: &str) -> Result<u16, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} expects an integer, got '{value}'"))
}

fn parse_u8(flag: &str, value: &str) -> Result<u8, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} expects an integer, got '{value}'"))
}
