//! Reproducible synthetic fixtures; all flights, operators and rarity signals are invented.
use std::{fs, path::PathBuf};

use serde::Serialize;

use crate::domain::{
    Airport, FlightCandidate, FlightIdentification, FlightPerformance, FlightTimings,
    SelectionFeatures,
};

const AIRPORTS: &[(&str, &str, f64, f64)] = &[
    ("LHR", "London Heathrow Airport", 51.47, -0.4543),
    ("KEF", "Keflavik International Airport", 63.985, -22.606),
    (
        "ANC",
        "Ted Stevens Anchorage International Airport",
        61.174,
        -149.999,
    ),
    ("FRA", "Frankfurt Airport", 50.038, 8.562),
    (
        "JFK",
        "John F Kennedy International Airport",
        40.641,
        -73.778,
    ),
    ("LAX", "Los Angeles International Airport", 33.942, -118.409),
    (
        "GRU",
        "Sao Paulo Guarulhos International Airport",
        -23.436,
        -46.473,
    ),
    (
        "SCL",
        "Arturo Merino Benitez International Airport",
        -33.393,
        -70.786,
    ),
    ("JNB", "O R Tambo International Airport", -26.137, 28.241),
    ("NBO", "Jomo Kenyatta International Airport", -1.319, 36.928),
    ("DXB", "Dubai International Airport", 25.253, 55.366),
    ("DEL", "Indira Gandhi International Airport", 28.556, 77.1),
    ("SIN", "Singapore Changi Airport", 1.364, 103.992),
    ("HND", "Tokyo Haneda Airport", 35.549, 139.78),
    ("SYD", "Sydney Kingsford Smith Airport", -33.94, 151.175),
    ("AKL", "Auckland Airport", -37.008, 174.792),
];
const AIRCRAFT: &[&str] = &[
    "A320NEO",
    "B737-800",
    "B747-8F",
    "B787-9",
    "A350-900",
    "A380-800",
    "B777-300ER",
    "A330-900",
];

#[derive(Serialize)]
struct Fixture {
    date: String,
    synthetic: bool,
    description: &'static str,
    flights: Vec<FlightCandidate>,
}

pub fn run(args: &[String]) -> Result<(), String> {
    let mut output = PathBuf::from("fixtures/generated");
    let mut days = 30_u32;
    let mut count = 500_u32;
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        if matches!(flag.as_str(), "--help" | "-h") {
            println!(
                "generate-test-data [--output DIR] [--days 1..365] [--flights-per-day 10..10000]\nStarts 2026-10-01. Defaults: 30 days, 500 flights/day, fixtures/generated."
            );
            return Ok(());
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        match flag.as_str() {
            "--output" => output = value.into(),
            "--days" => days = value.parse().map_err(|_| "invalid --days")?,
            "--flights-per-day" => {
                count = value.parse().map_err(|_| "invalid --flights-per-day")?
            }
            _ => return Err(format!("unknown generation option {flag}")),
        }
    }
    if !(1..=365).contains(&days) || !(10..=10_000).contains(&count) {
        return Err("days must be 1..365 and flights-per-day must be 10..10000".into());
    }
    fs::create_dir_all(&output).map_err(|error| error.to_string())?;
    let mut date = (2026, 10, 1);
    for day in 0..days {
        let date_text = format!("{:04}-{:02}-{:02}", date.0, date.1, date.2);
        let fixture = generate(&date_text, day, count);
        let bytes = serde_json::to_vec_pretty(&fixture).map_err(|error| error.to_string())?;
        fs::write(output.join(format!("{date_text}.json")), bytes)
            .map_err(|error| error.to_string())?;
        next_date(&mut date);
    }
    println!(
        "Generated {} synthetic flights across {days} days in {}",
        days * count,
        output.display()
    );
    Ok(())
}

fn generate(date: &str, day: u32, count: u32) -> Fixture {
    // Fixed seed per day: rerunning the generator reproduces every byte.
    let mut random = Random(u64::from(day) + 42);
    let flights = (0..count)
        .map(|index| {
            let origin_index = random.below(AIRPORTS.len() as u32) as usize;
            let destination_index =
                (origin_index + 1 + random.below((AIRPORTS.len() - 1) as u32) as usize)
                    % AIRPORTS.len();
            let origin = airport(origin_index);
            let destination = airport(destination_index);
            let distance_km = great_circle_km(&origin, &destination);
            let direct_nm = (f64::from(distance_km) / 1.852).round() as u32;
            let track_nm = direct_nm * (100 + random.below(20)) / 100;
            let duration = (track_nm * 60 / 450).max(25);
            let scheduled_dep = random.below(1440);
            let departure_delay = random.below(90);
            let arrival_delay = departure_delay + random.below(30);
            let actual_dep = scheduled_dep + departure_delay;
            let actual_arr = actual_dep + duration;
            let scheduled_arr = actual_arr - arrival_delay;
            let mut flight = FlightCandidate {
                provider: "synthetic".into(),
                // IDs deliberately repeat across days to exercise date-aware storage.
                provider_flight_id: format!("test-{index:05}"),
                date: date.into(),
                callsign: format!("TST{index:04}"),
                aircraft_type: AIRCRAFT[random.below(AIRCRAFT.len() as u32) as usize].into(),
                identification: FlightIdentification {
                    flight_number: Some(format!("DF{index:04}")),
                    airline: Some(format!("DEMO AIR {}", 1 + random.below(8))),
                    registration: Some(format!("X-{index:04}")),
                    status: Some("ARRIVED".into()),
                },
                origin,
                destination,
                distance_km,
                timings: FlightTimings {
                    scheduled_departure_utc: Some(time(scheduled_dep)),
                    actual_departure_utc: Some(time(actual_dep)),
                    scheduled_arrival_utc: Some(time(scheduled_arr)),
                    actual_arrival_utc: Some(time(actual_arr)),
                    duration_minutes: Some(duration),
                    arrival_delay_minutes: Some(arrival_delay as i32),
                },
                performance: FlightPerformance {
                    cruise_altitude_ft: Some(30_000 + random.below(12) * 1000),
                    max_ground_speed_knots: Some(460 + random.below(100)),
                    direct_distance_nm: Some(direct_nm),
                    track_distance_nm: Some(track_nm),
                },
                features: SelectionFeatures {
                    aircraft_rarity: random.below(101) as u8,
                    route_rarity: random.below(101) as u8,
                    geographic_interest: random.below(101) as u8,
                    data_quality: (60 + random.below(41)) as u8,
                },
            };
            match index % 50 {
                0 => {
                    flight.destination = flight.origin.clone();
                    flight.distance_km = 2;
                }
                1 => flight.origin.latitude = 95.0,
                2 => flight.features.route_rarity = 101,
                3 => {
                    flight.identification = FlightIdentification::default();
                    flight.timings = FlightTimings::default();
                    flight.performance = FlightPerformance::default();
                }
                _ => {}
            }
            flight
        })
        .collect();
    Fixture {
        date: date.into(),
        synthetic: true,
        description: "Invented test flights. Times are UTC clock values modulo 24h; rarity signals are synthetic, not measured. Includes invalid candidates and missing optional fields.",
        flights,
    }
}

pub fn candidates(date: &str, seed: u32, count: u32) -> Vec<FlightCandidate> {
    generate(date, seed, count).flights
}

struct Random(u64);
impl Random {
    fn below(&mut self, limit: u32) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((self.0 >> 32) as u32) % limit
    }
}
fn airport(index: usize) -> Airport {
    let (code, name, latitude, longitude) = AIRPORTS[index];
    Airport {
        code: code.into(),
        name: name.into(),
        latitude,
        longitude,
    }
}
fn great_circle_km(a: &Airport, b: &Airport) -> u32 {
    let latitude = ((b.latitude - a.latitude).to_radians() / 2.0).sin().powi(2);
    let longitude = ((b.longitude - a.longitude).to_radians() / 2.0)
        .sin()
        .powi(2);
    let haversine =
        latitude + a.latitude.to_radians().cos() * b.latitude.to_radians().cos() * longitude;
    (6371.0 * 2.0 * haversine.clamp(0.0, 1.0).sqrt().asin()).round() as u32
}
fn time(minutes: u32) -> String {
    format!("{:02}:{:02}", minutes / 60 % 24, minutes % 60)
}
fn next_date(date: &mut (u32, u32, u32)) {
    let leap =
        date.0.is_multiple_of(4) && (!date.0.is_multiple_of(100) || date.0.is_multiple_of(400));
    let days = match date.1 {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    date.2 += 1;
    if date.2 > days {
        date.2 = 1;
        date.1 += 1;
    }
    if date.1 > 12 {
        date.1 = 1;
        date.0 += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_data_is_repeatable_and_exercises_filtering() {
        let first = generate("2026-10-01", 0, 500);
        let second = generate("2026-10-01", 0, 500);
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&second).unwrap()
        );
        let scored = crate::scoring::score_candidates(first.flights).unwrap();
        assert_eq!(scored.len(), 470);
        let mut next = (2026, 12, 31);
        next_date(&mut next);
        assert_eq!(next, (2027, 1, 1));
    }
}
