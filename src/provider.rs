use std::{fmt, fs, io, path::Path};

use serde::Deserialize;

use crate::domain::FlightCandidate;

pub trait FlightProvider {
    fn name(&self) -> &str;
    fn flights_for_date(&self, date: &str) -> Result<Vec<FlightCandidate>, ProviderError>;
}

pub struct FixtureProvider {
    fixture: FixtureFile,
}

impl FixtureProvider {
    pub fn synthetic(date: &str, seed: u32) -> Self {
        Self {
            fixture: FixtureFile {
                date: date.to_owned(),
                flights: crate::test_data::candidates(date, seed, 500),
            },
        }
    }
    pub fn from_path(path: &Path) -> Result<Self, ProviderError> {
        let json = fs::read(path).map_err(ProviderError::Io)?;
        Self::from_json(&json)
    }

    fn from_json(json: &[u8]) -> Result<Self, ProviderError> {
        let fixture = serde_json::from_slice(json).map_err(ProviderError::Json)?;
        Ok(Self { fixture })
    }

    pub fn date(&self) -> &str {
        &self.fixture.date
    }
}

impl FlightProvider for FixtureProvider {
    fn name(&self) -> &str {
        "fixture"
    }

    fn flights_for_date(&self, date: &str) -> Result<Vec<FlightCandidate>, ProviderError> {
        if self.fixture.date != date {
            return Err(ProviderError::DateMismatch {
                requested: date.to_owned(),
                fixture: self.fixture.date.clone(),
            });
        }
        Ok(self
            .fixture
            .flights
            .iter()
            .filter(|flight| flight.date == date)
            .cloned()
            .collect())
    }
}

#[derive(Deserialize)]
struct FixtureFile {
    date: String,
    flights: Vec<FlightCandidate>,
}

#[derive(Debug)]
pub enum ProviderError {
    Io(io::Error),
    Json(serde_json::Error),
    DateMismatch { requested: String, fixture: String },
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "could not read fixture: {error}"),
            Self::Json(error) => write!(formatter, "invalid fixture JSON: {error}"),
            Self::DateMismatch { requested, fixture } => write!(
                formatter,
                "fixture contains date {fixture}, but {requested} was requested"
            ),
        }
    }
}

impl std::error::Error for ProviderError {}

#[cfg(test)]
mod tests {
    use crate::scoring::select_most_interesting;

    use super::*;

    const EMPTY_FIXTURE: &[u8] = br#"{"date":"2026-10-29","flights":[]}"#;

    #[test]
    fn fixture_provider_rejects_a_different_date() {
        let provider = FixtureProvider::from_json(EMPTY_FIXTURE).unwrap();
        let error = provider.flights_for_date("2026-10-30").unwrap_err();

        assert!(matches!(error, ProviderError::DateMismatch { .. }));
    }

    #[test]
    fn project_fixture_selects_the_expected_flight() {
        let provider =
            FixtureProvider::from_json(include_bytes!("../fixtures/flights.json")).unwrap();
        let flights = provider.flights_for_date(provider.date()).unwrap();
        let selected = select_most_interesting(flights).unwrap();

        assert_eq!(selected.flight.provider_flight_id, "global-001");
        assert_eq!(selected.score.algorithm_version, "mixed-interest-v1");
    }
}
