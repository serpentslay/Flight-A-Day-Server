use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FlightCandidate {
    pub provider: String,
    pub provider_flight_id: String,
    pub date: String,
    pub callsign: String,
    pub aircraft_type: String,
    #[serde(default)]
    pub identification: FlightIdentification,
    pub origin: Airport,
    pub destination: Airport,
    pub distance_km: u32,
    #[serde(default)]
    pub timings: FlightTimings,
    #[serde(default)]
    pub performance: FlightPerformance,
    pub features: SelectionFeatures,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct FlightIdentification {
    pub flight_number: Option<String>,
    pub airline: Option<String>,
    pub registration: Option<String>,
    pub status: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct FlightTimings {
    pub scheduled_departure_utc: Option<String>,
    pub actual_departure_utc: Option<String>,
    pub scheduled_arrival_utc: Option<String>,
    pub actual_arrival_utc: Option<String>,
    pub duration_minutes: Option<u32>,
    pub arrival_delay_minutes: Option<i32>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct FlightPerformance {
    pub cruise_altitude_ft: Option<u32>,
    pub max_ground_speed_knots: Option<u32>,
    pub direct_distance_nm: Option<u32>,
    pub track_distance_nm: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Airport {
    pub code: String,
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SelectionFeatures {
    /// Normalised upstream enrichment signals in the inclusive range 0..=100.
    pub aircraft_rarity: u8,
    pub route_rarity: u8,
    pub geographic_interest: u8,
    pub data_quality: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ScoreBreakdown {
    pub algorithm_version: &'static str,
    pub total: u32,
    pub aircraft_rarity: u32,
    pub route_rarity: u32,
    pub geographic_interest: u32,
    pub distance: u32,
    pub data_quality: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SelectedFlight {
    pub flight: FlightCandidate,
    pub score: ScoreBreakdown,
    pub explanation: String,
}
