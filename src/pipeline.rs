use std::{collections::HashMap, fs, path::Path};

use sha2::{Digest, Sha256};

use crate::{
    artifact::{ArtifactPaths, DisplayProfile, RENDERER_VERSION, render_selected_flight},
    domain::SelectedFlight,
    provider::FlightProvider,
    scoring::score_candidates,
    storage::{Storage, StoredArtifact},
};

pub struct RunResult {
    pub selected: SelectedFlight,
    pub candidate_count: usize,
    pub display_number: String,
    pub artifacts: ArtifactPaths,
    pub artifacts_reused: bool,
}

pub async fn run_fixture_pipeline(
    storage: &Storage,
    provider: &dyn FlightProvider,
    date: &str,
    output_directory: &Path,
    profile: DisplayProfile,
) -> Result<RunResult, String> {
    let lease_token = storage
        .begin_run(date)
        .await
        .map_err(|error| error.to_string())?;
    let result = run_inner(
        storage,
        provider,
        date,
        &lease_token,
        output_directory,
        profile,
    )
    .await;
    match &result {
        Ok(_) => storage
            .complete_run(date, &lease_token)
            .await
            .map_err(|error| error.to_string())?,
        Err(error) => {
            if let Err(storage_error) = storage.fail_run(date, &lease_token, error).await {
                return Err(format!(
                    "{error}; additionally failed to record error: {storage_error}"
                ));
            }
        }
    }
    result
}

async fn run_inner(
    storage: &Storage,
    provider: &dyn FlightProvider,
    date: &str,
    lease_token: &str,
    output_directory: &Path,
    profile: DisplayProfile,
) -> Result<RunResult, String> {
    let candidates = provider
        .flights_for_date(date)
        .map_err(|error| error.to_string())?;
    let candidate_count = candidates.len();
    let mut flight_ids = HashMap::new();
    for flight in &candidates {
        let id = storage
            .upsert_flight(flight)
            .await
            .map_err(|error| error.to_string())?;
        flight_ids.insert(flight_key(flight), id);
    }
    storage
        .set_stage(date, lease_token, "collected")
        .await
        .map_err(|error| error.to_string())?;

    let scored = score_candidates(candidates).map_err(|error| error.to_string())?;
    let selected = scored
        .first()
        .cloned()
        .ok_or_else(|| "scoring returned no candidates".to_owned())?;
    let mut stored_scores = Vec::with_capacity(scored.len());
    for candidate in &scored {
        let key = flight_key(&candidate.flight);
        let flight_id = *flight_ids
            .get(&key)
            .ok_or_else(|| format!("stored flight id missing for {key}"))?;
        stored_scores.push((flight_id, candidate));
    }
    let selected_key = flight_key(&selected.flight);
    let selected_id = *flight_ids
        .get(&selected_key)
        .ok_or_else(|| format!("stored selected flight id missing for {selected_key}"))?;
    let display_number = storage
        .record_scoring_and_selection(date, &stored_scores, selected_id, &selected, "000")
        .await
        .map_err(|error| error.to_string())?;
    storage
        .set_stage(date, lease_token, "scored")
        .await
        .map_err(|error| error.to_string())?;
    storage
        .set_stage(date, lease_token, "selected")
        .await
        .map_err(|error| error.to_string())?;

    let input_hash = render_input_hash(&selected, profile, &display_number)?;
    let dated_output = output_directory.join(date);
    let paths = ArtifactPaths {
        binary: dated_output.join("display.bin"),
        preview: dated_output.join("preview.png"),
        manifest: dated_output.join("flight.json"),
    };
    let stored_artifacts = storage
        .artifacts_for_date(date)
        .await
        .map_err(|error| error.to_string())?;
    let artifacts_reused = artifacts_are_valid(&stored_artifacts, &paths, &input_hash);

    if !artifacts_reused {
        let rendered = render_selected_flight(&dated_output, profile, &selected, &display_number)
            .map_err(|error| error.to_string())?;
        for (kind, path) in [
            ("binary", &rendered.binary),
            ("preview", &rendered.preview),
            ("manifest", &rendered.manifest),
        ] {
            let artifact = artifact_record(kind, path, &input_hash)?;
            storage
                .record_artifact(date, &artifact)
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    storage
        .set_stage(date, lease_token, "rendered")
        .await
        .map_err(|error| error.to_string())?;

    Ok(RunResult {
        selected,
        candidate_count,
        display_number,
        artifacts: paths,
        artifacts_reused,
    })
}

fn flight_key(flight: &crate::domain::FlightCandidate) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}",
        flight.provider, flight.provider_flight_id, flight.date
    )
}

fn render_input_hash(
    selected: &SelectedFlight,
    profile: DisplayProfile,
    display_number: &str,
) -> Result<String, String> {
    let bytes = serde_json::to_vec(&(selected, profile, display_number, RENDERER_VERSION))
        .map_err(|error| error.to_string())?;
    Ok(sha256(&bytes))
}

fn artifact_record(kind: &str, path: &Path, input_hash: &str) -> Result<StoredArtifact, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
    Ok(StoredArtifact {
        kind: kind.to_owned(),
        path: path.to_string_lossy().into_owned(),
        size_bytes: bytes.len() as i64,
        sha256: sha256(&bytes),
        renderer_version: RENDERER_VERSION.to_owned(),
        input_hash: input_hash.to_owned(),
    })
}

fn artifacts_are_valid(
    artifacts: &[StoredArtifact],
    paths: &ArtifactPaths,
    input_hash: &str,
) -> bool {
    let expected = [
        ("binary", &paths.binary),
        ("preview", &paths.preview),
        ("manifest", &paths.manifest),
    ];
    expected.iter().all(|(kind, path)| {
        let Some(stored) = artifacts.iter().find(|artifact| artifact.kind == *kind) else {
            return false;
        };
        if stored.renderer_version != RENDERER_VERSION
            || stored.input_hash != input_hash
            || stored.path != path.to_string_lossy()
        {
            return false;
        }
        let Ok(bytes) = fs::read(path) else {
            return false;
        };
        stored.size_bytes == bytes.len() as i64 && stored.sha256 == sha256(&bytes)
    })
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::{
        domain::FlightCandidate,
        provider::{FixtureProvider, ProviderError},
    };

    use super::*;

    struct TestProvider {
        flights: Vec<FlightCandidate>,
    }

    impl FlightProvider for TestProvider {
        fn name(&self) -> &str {
            "test"
        }

        fn flights_for_date(&self, _date: &str) -> Result<Vec<FlightCandidate>, ProviderError> {
            Ok(self.flights.clone())
        }
    }

    async fn remove_test_directory(path: &Path) {
        for attempt in 0..40 {
            match fs::remove_dir_all(path) {
                Ok(()) => return,
                Err(_) if attempt < 39 => {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                Err(error) => panic!("remove pipeline test directory: {error}"),
            }
        }
    }

    #[tokio::test]
    async fn fixture_pipeline_is_persistent_idempotent_and_repairs_artifacts() {
        let root =
            std::env::temp_dir().join(format!("flight-a-day-pipeline-test-{}", std::process::id()));
        if root.exists() {
            remove_test_directory(&root).await;
        }
        let database = root.join("data/flight-a-day.db");
        let output = root.join("out");
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/flights.json");
        let provider = FixtureProvider::from_path(&fixture).expect("load fixture provider");
        let storage = Storage::connect(&database)
            .await
            .expect("open test database");

        let first = run_fixture_pipeline(
            &storage,
            &provider,
            provider.date(),
            &output,
            DisplayProfile::default(),
        )
        .await
        .expect("first pipeline run");
        assert!(!first.artifacts_reused);
        assert_eq!(first.display_number, "000");
        assert_eq!(fs::metadata(&first.artifacts.binary).unwrap().len(), 48_000);
        assert_eq!(storage.flight_count().await.unwrap(), 6);

        let second = run_fixture_pipeline(
            &storage,
            &provider,
            provider.date(),
            &output,
            DisplayProfile::default(),
        )
        .await
        .expect("second pipeline run");
        assert!(second.artifacts_reused);
        assert_eq!(storage.flight_count().await.unwrap(), 6);
        let status = storage
            .status(Some(provider.date()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status.status, "succeeded");
        assert_eq!(status.stage, "completed");
        assert_eq!(status.attempt_count, 2);
        assert_eq!(status.display_number.as_deref(), Some("000"));

        fs::remove_file(&second.artifacts.binary).expect("delete binary to test repair");
        let repaired = run_fixture_pipeline(
            &storage,
            &provider,
            provider.date(),
            &output,
            DisplayProfile::default(),
        )
        .await
        .expect("repair pipeline run");
        assert!(!repaired.artifacts_reused);
        assert_eq!(
            fs::metadata(&repaired.artifacts.binary).unwrap().len(),
            48_000
        );

        let mut next_day_flight = repaired.selected.flight.clone();
        next_day_flight.date = "2026-10-30".into();
        let next_day_provider = TestProvider {
            flights: vec![next_day_flight],
        };
        let next_day = run_fixture_pipeline(
            &storage,
            &next_day_provider,
            "2026-10-30",
            &output,
            DisplayProfile::default(),
        )
        .await
        .expect("next-day pipeline run");
        assert_eq!(storage.flight_count().await.unwrap(), 7);
        assert!(repaired.artifacts.binary.exists());
        assert!(next_day.artifacts.binary.exists());
        assert_ne!(repaired.artifacts.binary, next_day.artifacts.binary);

        let failed = run_fixture_pipeline(
            &storage,
            &provider,
            "2026-10-31",
            &output,
            DisplayProfile::default(),
        )
        .await;
        assert!(failed.is_err());
        let failed_status = storage.status(Some("2026-10-31")).await.unwrap().unwrap();
        assert_eq!(failed_status.status, "failed");
        assert!(failed_status.error.is_some());

        let lease_token = storage.begin_run("2026-11-01").await.unwrap();
        let overlapping = storage.begin_run("2026-11-01").await;
        assert!(overlapping.is_err());
        let wrong_owner = storage
            .set_stage("2026-11-01", "not-the-owner", "collected")
            .await;
        assert!(wrong_owner.is_err());
        storage
            .fail_run("2026-11-01", &lease_token, "test cleanup")
            .await
            .unwrap();

        storage.close().await;
        remove_test_directory(&root).await;
    }
}
