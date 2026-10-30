use std::{
    env,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use chrono::{DateTime, Datelike, Days, NaiveDate, NaiveTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::{
    artifact::{self, ArtifactPaths, DisplayProfile},
    pipeline,
    provider::{FixtureProvider, FlightProvider},
    publishing, scoring,
    storage::Storage,
};

pub struct Config {
    data: PathBuf,
    output: PathBuf,
    fixtures: PathBuf,
    source: String,
    run_time: NaiveTime,
    start: Option<NaiveDate>,
    poll_seconds: u64,
    publish_enabled: bool,
}

impl Config {
    fn load() -> Result<Self, String> {
        let data = PathBuf::from(setting("FLIGHT_DATA_DIR", "data"));
        let source = setting("FLIGHT_SOURCE", "synthetic");
        if !matches!(source.as_str(), "synthetic" | "fixtures") {
            return Err("FLIGHT_SOURCE must be synthetic or fixtures".into());
        }
        let run_time = NaiveTime::parse_from_str(&setting("FLIGHT_RUN_TIME", "04:00"), "%H:%M")
            .map_err(|_| "FLIGHT_RUN_TIME must be HH:MM in UTC")?;
        let start = env::var("FLIGHT_START_DATE")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|value| parse_date(&value))
            .transpose()?;
        let poll_seconds = setting("FLIGHT_POLL_SECONDS", "60")
            .parse::<u64>()
            .map_err(|_| "invalid FLIGHT_POLL_SECONDS")?;
        if !(1..=60).contains(&poll_seconds) {
            return Err("FLIGHT_POLL_SECONDS must be 1..60".into());
        }
        Ok(Self {
            output: env::var_os("FLIGHT_OUTPUT_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| data.join("artifacts")),
            fixtures: PathBuf::from(setting("FLIGHT_FIXTURE_DIR", "fixtures/generated")),
            data,
            source,
            run_time,
            start,
            poll_seconds,
            publish_enabled: publishing::enabled()?,
        })
    }

    fn provider(&self, date: NaiveDate) -> Result<FixtureProvider, String> {
        if self.source == "synthetic" {
            Ok(FixtureProvider::synthetic(
                &date.to_string(),
                date.num_days_from_ce() as u32,
            ))
        } else {
            FixtureProvider::from_path(&self.fixtures.join(format!("{date}.json")))
                .map_err(|e| e.to_string())
        }
    }

    async fn storage(&self) -> Result<Storage, String> {
        Storage::connect(&self.data.join("flight-a-day.db"))
            .await
            .map_err(|e| e.to_string())
    }
}

fn setting(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.into())
}

pub fn parse_date(value: &str) -> Result<NaiveDate, String> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| format!("invalid date: {value}; expected YYYY-MM-DD"))?;
    if date.to_string() != value {
        return Err("date must use YYYY-MM-DD".into());
    }
    Ok(date)
}

pub async fn run(args: &[String]) -> Result<(), String> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        println!(
            "serve [--once]\nrun-now [--date YYYY-MM-DD]\nrefresh-test [--date YYYY-MM-DD] [--publish]\npublish [--date YYYY-MM-DD | --test]\npublication-status\nhealth\nConfiguration: FLIGHT_DATA_DIR, FLIGHT_OUTPUT_DIR, FLIGHT_SOURCE=synthetic|fixtures, FLIGHT_FIXTURE_DIR, FLIGHT_RUN_TIME=04:00 (UTC), FLIGHT_START_DATE, FLIGHT_POLL_SECONDS=60, FLIGHT_PUBLISH_ENABLED=false.\nR2: R2_ENDPOINT, R2_BUCKET, R2_PUBLIC_BASE_URL, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY (credentials also support _FILE).\nOffline publishing: FLIGHT_PUBLISH_BACKEND=local."
        );
        return Ok(());
    }
    let config = Config::load()?;
    match args[0].as_str() {
        "serve"
            if args.len() == 1 || args.get(1).is_some_and(|a| a == "--once") && args.len() == 2 =>
        {
            serve(&config, args.len() == 2).await
        }
        "health" if args.len() == 1 => health(&config),
        "publication-status" if args.len() == 1 => {
            let storage = config.storage().await?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &storage
                        .publication_status()
                        .await
                        .map_err(|e| e.to_string())?
                )
                .map_err(|e| e.to_string())?
            );
            Ok(())
        }
        "publish" => {
            let test = args.get(1).is_some_and(|arg| arg == "--test") && args.len() == 2;
            let _lock = lock(
                &config
                    .data
                    .join(if test { "test.lock" } else { "daily.lock" }),
            )?;
            let storage = config.storage().await?;
            if test {
                let sequence = storage
                    .state("test_sequence")
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or("no test image exists; run refresh-test first")?;
                let revision = format!(
                    "{:06}",
                    sequence
                        .parse::<u64>()
                        .map_err(|_| "invalid test sequence")?
                );
                publish_channel(&config, &storage, "test", &revision).await
            } else {
                let date = match &args[1..] {
                    [flag, value] if flag == "--date" => parse_date(value)?.to_string(),
                    [] => storage
                        .state("latest_daily")
                        .await
                        .map_err(|e| e.to_string())?
                        .ok_or("no daily image exists; run run-now first")?,
                    _ => return Err("publish expects --date YYYY-MM-DD or --test".into()),
                };
                publish_channel(&config, &storage, "daily", &date).await
            }
        }
        "run-now" | "refresh-test" => {
            let publish = args[0] == "refresh-test" && args.iter().any(|arg| arg == "--publish");
            let remaining: Vec<_> = args[1..]
                .iter()
                .filter(|arg| !(publish && arg.as_str() == "--publish"))
                .cloned()
                .collect();
            let date = match &remaining[..] {
                [] => Utc::now().date_naive().pred_opt().ok_or("date overflow")?,
                [flag, date] if flag == "--date" => parse_date(date)?,
                _ => return Err("expected only --date YYYY-MM-DD".into()),
            };
            if args[0] == "refresh-test" {
                refresh_test(&config, date, publish).await
            } else {
                let _lock = lock(&config.data.join("daily.lock"))?;
                let storage = config.storage().await?;
                daily(&config, &storage, date).await
            }
        }
        _ => Err("invalid service command or arguments; use --help".into()),
    }
}

fn lock(path: &Path) -> Result<File, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.try_lock_exclusive()
        .map_err(|e| format!("another process owns {}: {e}", path.display()))?;
    Ok(file)
}

// Publish the pointer last. Files within each dated/revision directory are complete first.
fn publish_pointer(root: &Path, relative_directory: &str) -> Result<(), String> {
    let manifest =
        fs::read(root.join(relative_directory).join("flight.json")).map_err(|e| e.to_string())?;
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&manifest).map_err(|e| e.to_string())?;
    manifest["artifacts"]["binary"] = format!("{relative_directory}/display.bin").into();
    manifest["artifacts"]["preview"] = format!("{relative_directory}/preview.png").into();
    let bytes = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    atomic_write(&root.join("latest.json"), &bytes)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
    fs::rename(&temporary, path).map_err(|e| format!("could not replace {}: {e}", path.display()))
}

async fn daily(config: &Config, storage: &Storage, date: NaiveDate) -> Result<(), String> {
    let provider = config.provider(date)?;
    let result = pipeline::run_fixture_pipeline(
        storage,
        &provider,
        &date.to_string(),
        &config.output,
        DisplayProfile::default(),
    )
    .await?;
    if config.publish_enabled {
        publish_channel(config, storage, "daily", &date.to_string()).await?;
    }
    // Backfills must not move the latest pointer backwards.
    let previous = storage
        .state("latest_daily")
        .await
        .map_err(|e| e.to_string())?;
    if previous
        .as_deref()
        .is_none_or(|previous| previous <= date.to_string().as_str())
    {
        publish_pointer(&config.output, &date.to_string())?;
        storage
            .set_state("latest_daily", &date.to_string())
            .await
            .map_err(|e| e.to_string())?;
    }
    println!(
        "daily date={date} flight={} reused={} manifest={}",
        result.selected.flight.callsign,
        result.artifacts_reused,
        result.artifacts.manifest.display()
    );
    Ok(())
}

async fn refresh_test(config: &Config, date: NaiveDate, publish: bool) -> Result<(), String> {
    let _lock = lock(&config.data.join("test.lock"))?;
    let storage = config.storage().await?;
    let sequence = storage
        .state("test_sequence")
        .await
        .map_err(|e| e.to_string())?
        .map(|value| value.parse::<u64>().map_err(|e| e.to_string()))
        .transpose()?
        .unwrap_or(0)
        + 1;
    // Always synthetic so this works on any day, even without saved fixture files.
    let provider = FixtureProvider::synthetic(&date.to_string(), date.num_days_from_ce() as u32);
    let candidates = provider
        .flights_for_date(&date.to_string())
        .map_err(|e| e.to_string())?;
    let scored = scoring::score_candidates(candidates).map_err(|e| e.to_string())?;
    let selected = &scored[((sequence - 1) % scored.len() as u64) as usize];
    let root = config.output.join("test");
    let revision = format!("{sequence:06}");
    // Persist before writing: a crash consumes the number instead of reusing published paths.
    storage
        .set_state("test_sequence", &sequence.to_string())
        .await
        .map_err(|e| e.to_string())?;
    let paths = artifact::render_selected_flight(
        &root.join(&revision),
        DisplayProfile::default(),
        selected,
        &format!("{sequence:03}"),
    )
    .map_err(|e| e.to_string())?;
    publish_pointer(&root, &revision)?;
    if publish || config.publish_enabled {
        publish_channel(config, &storage, "test", &revision).await?;
    }
    print_test_paths(&paths, &root);
    Ok(())
}

async fn publish_channel(
    config: &Config,
    storage: &Storage,
    channel: &str,
    revision: &str,
) -> Result<(), String> {
    let directory = if channel == "test" {
        config.output.join("test").join(revision)
    } else {
        config.output.join(revision)
    };
    publishing::publish_from_env(storage, &directory, channel, revision, &config.data).await
}

fn print_test_paths(paths: &ArtifactPaths, root: &Path) {
    println!(
        "Test image generated immediately. Read {} for the latest binary path and checksum.",
        root.join("latest.json").display()
    );
    println!(
        "BIN: {}\nPNG: {}",
        paths.binary.display(),
        paths.preview.display()
    );
}

fn due_date(now: DateTime<Utc>, run_time: NaiveTime) -> NaiveDate {
    now.date_naive()
        .checked_sub_days(Days::new(if now.time() >= run_time { 1 } else { 2 }))
        .expect("current UTC date is in range")
}

#[derive(Serialize, Deserialize)]
struct Health {
    heartbeat: DateTime<Utc>,
    target: NaiveDate,
    completed: Option<NaiveDate>,
    error: Option<String>,
}

async fn serve(config: &Config, once: bool) -> Result<(), String> {
    let _lock = lock(&config.data.join("daily.lock"))?;
    let storage = config.storage().await?;
    let stopping = Arc::new(AtomicBool::new(false));
    let signal_flag = Arc::clone(&stopping);
    let signal = tokio::spawn(async move {
        shutdown_signal().await;
        signal_flag.store(true, Ordering::SeqCst);
    });
    let mut cursor = storage
        .state("scheduler_cursor")
        .await
        .map_err(|e| e.to_string())?
        .map(|date| parse_date(&date))
        .transpose()?;
    println!(
        "scheduler started source={} run_time={} UTC",
        config.source, config.run_time
    );
    let mut publication_checked = None;
    loop {
        let target = due_date(Utc::now(), config.run_time);
        let mut next = cursor
            .and_then(|date| date.succ_opt())
            .or(config.start)
            .unwrap_or(target);
        let mut last_error = None;
        while next <= target && !stopping.load(Ordering::SeqCst) {
            write_health(config, target, cursor, None)?;
            if let Err(error) = daily(config, &storage, next).await {
                eprintln!("daily date={next} error={error}");
                last_error = Some(error);
                break;
            }
            storage
                .set_state("scheduler_cursor", &next.to_string())
                .await
                .map_err(|e| e.to_string())?;
            cursor = Some(next);
            if config.publish_enabled {
                publication_checked = cursor;
            }
            next = next.succ_opt().ok_or("date overflow")?;
        }
        // Enabling publishing after local-only runs must publish the saved latest day too.
        if config.publish_enabled
            && publication_checked != cursor
            && last_error.is_none()
            && !stopping.load(Ordering::SeqCst)
            && let Some(date) = cursor
        {
            match publish_channel(config, &storage, "daily", &date.to_string()).await {
                Ok(()) => publication_checked = cursor,
                Err(error) => {
                    eprintln!("publication date={date} error={error}");
                    last_error = Some(error);
                }
            }
        }
        write_health(config, target, cursor, last_error.clone())?;
        if once || stopping.load(Ordering::SeqCst) {
            signal.abort();
            return last_error.map_or(Ok(()), Err);
        }
        // Small sleeps let SIGTERM/Ctrl-C stop an idle scheduler promptly.
        for _ in 0..config.poll_seconds {
            if stopping.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

fn write_health(
    config: &Config,
    target: NaiveDate,
    completed: Option<NaiveDate>,
    error: Option<String>,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(&Health {
        heartbeat: Utc::now(),
        target,
        completed,
        error,
    })
    .map_err(|e| e.to_string())?;
    atomic_write(&config.data.join("health.json"), &bytes)
}

fn health(config: &Config) -> Result<(), String> {
    let bytes = fs::read(config.data.join("health.json"))
        .map_err(|e| format!("scheduler heartbeat unavailable: {e}"))?;
    let state: Health = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if (Utc::now() - state.heartbeat).num_seconds() > 180 {
        return Err("scheduler heartbeat is stale".into());
    }
    if let Some(error) = state.error {
        return Err(format!("daily pipeline failed: {error}"));
    }
    if state
        .completed
        .is_none_or(|date| date < due_date(Utc::now(), config.run_time))
    {
        return Err("daily pipeline is catching up".into());
    }
    println!("healthy: completed {}", state.completed.unwrap());
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utc_schedule_handles_midnight_and_run_time_boundary() {
        let time = NaiveTime::from_hms_opt(4, 0, 0).unwrap();
        for (now, expected) in [
            ("2026-11-01T03:59:59Z", "2026-10-30"),
            ("2026-11-01T04:00:00Z", "2026-10-31"),
        ] {
            assert_eq!(due_date(now.parse().unwrap(), time).to_string(), expected);
        }
        assert!(parse_date("../../bad").is_err());
        assert!(parse_date("2026-02-30").is_err());
    }

    #[tokio::test]
    async fn catch_up_restart_health_and_immediate_refresh() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!(
                "service-test-{}-{}",
                std::process::id(),
                Utc::now().timestamp_nanos_opt().unwrap()
            ));
        let target = Utc::now().date_naive().pred_opt().unwrap();
        let first = target.pred_opt().unwrap();
        let config = Config {
            data: root.join("data"),
            output: root.join("artifacts"),
            fixtures: root.join("fixtures"),
            source: "fixtures".into(),
            run_time: NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
            start: Some(first),
            poll_seconds: 1,
            publish_enabled: false,
        };
        fs::create_dir_all(&config.fixtures).unwrap();
        let mut fixture: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/flights.json")).unwrap();
        for date in [first, target] {
            fixture["date"] = date.to_string().into();
            for flight in fixture["flights"].as_array_mut().unwrap() {
                flight["date"] = date.to_string().into();
            }
            fs::write(
                config.fixtures.join(format!("{date}.json")),
                serde_json::to_vec(&fixture).unwrap(),
            )
            .unwrap();
        }
        assert!(health(&config).is_err());
        serve(&config, true).await.unwrap();
        health(&config).unwrap();
        let latest: serde_json::Value =
            serde_json::from_slice(&fs::read(config.output.join("latest.json")).unwrap()).unwrap();
        assert_eq!(latest["date"], target.to_string());
        serve(&config, true).await.unwrap();
        let storage = config.storage().await.unwrap();
        assert_eq!(
            storage
                .status(Some(&target.to_string()))
                .await
                .unwrap()
                .unwrap()
                .attempt_count,
            1
        );
        assert_eq!(
            storage.state("scheduler_cursor").await.unwrap().unwrap(),
            target.to_string()
        );

        refresh_test(&config, target, false).await.unwrap();
        let first_binary = fs::read(config.output.join("test/000001/display.bin")).unwrap();
        refresh_test(&config, target, false).await.unwrap();
        let second_binary = fs::read(config.output.join("test/000002/display.bin")).unwrap();
        assert_eq!(first_binary.len(), 48_000);
        assert_ne!(first_binary, second_binary);
        let test_latest: serde_json::Value =
            serde_json::from_slice(&fs::read(config.output.join("test/latest.json")).unwrap())
                .unwrap();
        assert_eq!(test_latest["artifacts"]["binary"], "000002/display.bin");
        assert_eq!(test_latest["display"]["image_number"], "002");
        assert_eq!(
            storage.state("scheduler_cursor").await.unwrap().unwrap(),
            target.to_string()
        );
        let owner = lock(&config.data.join("daily.lock")).unwrap();
        assert!(lock(&config.data.join("daily.lock")).is_err());
        drop(owner);
        // A missing next input makes the next due run fail and health reports it.
        storage
            .set_state("scheduler_cursor", &first.to_string())
            .await
            .unwrap();
        fs::remove_file(config.fixtures.join(format!("{target}.json"))).unwrap();
        assert!(serve(&config, true).await.is_err());
        assert!(health(&config).is_err());
        storage.close().await;
        for attempt in 0..40 {
            match fs::remove_dir_all(&root) {
                Ok(()) => break,
                Err(_) if attempt < 39 => tokio::time::sleep(Duration::from_millis(100)).await,
                Err(error) => panic!("clean up service test: {error}"),
            }
        }
    }
}
