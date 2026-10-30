use std::fmt;

use crate::domain::{FlightCandidate, ScoreBreakdown, SelectedFlight};

const ALGORITHM_VERSION: &str = "mixed-interest-v1";

#[cfg(test)]
pub fn select_most_interesting(
    candidates: Vec<FlightCandidate>,
) -> Result<SelectedFlight, SelectionError> {
    Ok(score_candidates(candidates)?.remove(0))
}

pub fn score_candidates(
    candidates: Vec<FlightCandidate>,
) -> Result<Vec<SelectedFlight>, SelectionError> {
    let rejected = candidates
        .iter()
        .filter(|candidate| !is_valid(candidate))
        .count();
    let mut scored: Vec<_> = candidates.into_iter().filter(is_valid).map(score).collect();

    scored.sort_by(|left, right| {
        right
            .score
            .total
            .cmp(&left.score.total)
            .then_with(|| left.flight.provider.cmp(&right.flight.provider))
            .then_with(|| {
                left.flight
                    .provider_flight_id
                    .cmp(&right.flight.provider_flight_id)
            })
    });

    if scored.is_empty() {
        Err(SelectionError::NoValidCandidates { rejected })
    } else {
        Ok(scored)
    }
}

fn is_valid(candidate: &FlightCandidate) -> bool {
    !candidate.provider.trim().is_empty()
        && !candidate.provider_flight_id.trim().is_empty()
        && !candidate.date.trim().is_empty()
        && !candidate.callsign.trim().is_empty()
        && !candidate.aircraft_type.trim().is_empty()
        && !candidate.origin.code.trim().is_empty()
        && !candidate.destination.code.trim().is_empty()
        && candidate.origin.code != candidate.destination.code
        && (50..=25_000).contains(&candidate.distance_km)
        && valid_coordinates(candidate)
        && valid_features(candidate)
}

fn valid_coordinates(candidate: &FlightCandidate) -> bool {
    [&candidate.origin, &candidate.destination]
        .iter()
        .all(|airport| {
            (-90.0..=90.0).contains(&airport.latitude)
                && (-180.0..=180.0).contains(&airport.longitude)
        })
}

fn valid_features(candidate: &FlightCandidate) -> bool {
    let features = &candidate.features;
    [
        features.aircraft_rarity,
        features.route_rarity,
        features.geographic_interest,
        features.data_quality,
    ]
    .iter()
    .all(|value| *value <= 100)
}

fn score(flight: FlightCandidate) -> SelectedFlight {
    let distance_signal = flight.distance_km.min(15_000) * 100 / 15_000;
    let breakdown = ScoreBreakdown {
        algorithm_version: ALGORITHM_VERSION,
        aircraft_rarity: u32::from(flight.features.aircraft_rarity) * 25 / 100,
        route_rarity: u32::from(flight.features.route_rarity) * 25 / 100,
        geographic_interest: u32::from(flight.features.geographic_interest) * 20 / 100,
        distance: distance_signal * 15 / 100,
        data_quality: u32::from(flight.features.data_quality) * 15 / 100,
        total: 0,
    };
    let breakdown = ScoreBreakdown {
        total: breakdown.aircraft_rarity
            + breakdown.route_rarity
            + breakdown.geographic_interest
            + breakdown.distance
            + breakdown.data_quality,
        ..breakdown
    };
    let explanation = explanation(&breakdown);

    SelectedFlight {
        flight,
        score: breakdown,
        explanation,
    }
}

fn explanation(score: &ScoreBreakdown) -> String {
    let mut components = [
        (score.aircraft_rarity, "unusual aircraft type"),
        (score.route_rarity, "rare route"),
        (score.geographic_interest, "interesting geography"),
        (score.distance, "notable distance"),
        (score.data_quality, "high-quality flight data"),
    ];
    components.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(right.1)));
    format!(
        "Selected for its {} and {}.",
        components[0].1, components[1].1
    )
}

#[derive(Debug, PartialEq, Eq)]
pub enum SelectionError {
    NoValidCandidates { rejected: usize },
}

impl fmt::Display for SelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoValidCandidates { rejected } => {
                write!(
                    formatter,
                    "no valid flight candidates ({rejected} rejected)"
                )
            }
        }
    }
}

impl std::error::Error for SelectionError {}

#[cfg(test)]
mod tests {
    use crate::domain::{
        Airport, FlightIdentification, FlightPerformance, FlightTimings, SelectionFeatures,
    };

    use super::*;

    fn candidate(id: &str, route_rarity: u8) -> FlightCandidate {
        FlightCandidate {
            provider: "fixture".into(),
            provider_flight_id: id.into(),
            date: "2026-10-29".into(),
            callsign: id.into(),
            aircraft_type: "TEST TYPE".into(),
            identification: FlightIdentification::default(),
            origin: Airport {
                code: "AAA".into(),
                name: "Alpha".into(),
                latitude: 10.0,
                longitude: 10.0,
            },
            destination: Airport {
                code: "BBB".into(),
                name: "Bravo".into(),
                latitude: 20.0,
                longitude: 20.0,
            },
            distance_km: 1_000,
            timings: FlightTimings::default(),
            performance: FlightPerformance::default(),
            features: SelectionFeatures {
                aircraft_rarity: 50,
                route_rarity,
                geographic_interest: 50,
                data_quality: 90,
            },
        }
    }

    #[test]
    fn selection_is_independent_of_candidate_order() {
        let low = candidate("LOW", 10);
        let high = candidate("HIGH", 100);

        let forward = select_most_interesting(vec![low.clone(), high.clone()]).unwrap();
        let reverse = select_most_interesting(vec![high, low]).unwrap();

        assert_eq!(forward.flight.provider_flight_id, "HIGH");
        assert_eq!(forward, reverse);
    }

    #[test]
    fn invalid_candidates_are_filtered_out() {
        let valid = candidate("VALID", 50);
        let mut invalid = candidate("INVALID", 100);
        invalid.origin.code = invalid.destination.code.clone();

        let selected = select_most_interesting(vec![invalid, valid]).unwrap();

        assert_eq!(selected.flight.provider_flight_id, "VALID");
    }

    #[test]
    fn out_of_range_feature_is_filtered_out() {
        let valid = candidate("VALID", 50);
        let mut invalid = candidate("INVALID", 100);
        invalid.features.geographic_interest = 101;

        let selected = select_most_interesting(vec![invalid, valid]).unwrap();

        assert_eq!(selected.flight.provider_flight_id, "VALID");
    }

    #[test]
    fn stable_identifier_breaks_equal_score_ties() {
        let selected =
            select_most_interesting(vec![candidate("B", 50), candidate("A", 50)]).unwrap();

        assert_eq!(selected.flight.provider_flight_id, "A");
    }
}
