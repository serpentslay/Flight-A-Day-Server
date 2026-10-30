use std::{
    fs::{self, File},
    io::{self, BufWriter},
    path::{Path, PathBuf},
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::domain::{ScoreBreakdown, SelectedFlight};

const MIN_WIDTH: u32 = 320;
const MIN_HEIGHT: u32 = 200;
pub const RENDERER_VERSION: &str = "waveshare-daily-flight-v3";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct DisplayProfile {
    /// Native dimensions expected by the display controller.
    pub width: u32,
    pub height: u32,
    pub rotation_degrees: u16,
    pub black_bit: u8,
}

impl Default for DisplayProfile {
    fn default() -> Self {
        Self {
            width: 800,
            height: 480,
            rotation_degrees: 90,
            black_bit: 0,
        }
    }
}

impl DisplayProfile {
    fn validate(self) -> Result<(), ArtifactError> {
        if self.width < MIN_WIDTH || self.height < MIN_HEIGHT {
            return Err(ArtifactError::InvalidProfile(format!(
                "display must be at least {MIN_WIDTH}x{MIN_HEIGHT}"
            )));
        }
        if !self.width.is_multiple_of(8) {
            return Err(ArtifactError::InvalidProfile(
                "display width must be divisible by 8 for packed 1bpp output".into(),
            ));
        }
        if self.black_bit > 1 {
            return Err(ArtifactError::InvalidProfile(
                "black_bit must be 0 or 1".into(),
            ));
        }
        if !matches!(self.rotation_degrees, 0 | 90 | 180 | 270) {
            return Err(ArtifactError::InvalidProfile(
                "rotation_degrees must be 0, 90, 180, or 270".into(),
            ));
        }
        Ok(())
    }

    fn logical_dimensions(self) -> (u32, u32) {
        match self.rotation_degrees {
            90 | 270 => (self.height, self.width),
            _ => (self.width, self.height),
        }
    }
}

#[derive(Debug)]
pub enum ArtifactError {
    InvalidProfile(String),
    Io(io::Error),
    Png(png::EncodingError),
    Json(serde_json::Error),
}

impl std::fmt::Display for ArtifactError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidProfile(message) => {
                write!(formatter, "invalid display profile: {message}")
            }
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::Png(error) => write!(formatter, "PNG encoding error: {error}"),
            Self::Json(error) => write!(formatter, "JSON encoding error: {error}"),
        }
    }
}

impl std::error::Error for ArtifactError {}

impl From<io::Error> for ArtifactError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<png::EncodingError> for ArtifactError {
    fn from(value: png::EncodingError) -> Self {
        Self::Png(value)
    }
}

impl From<serde_json::Error> for ArtifactError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ArtifactPaths {
    pub binary: PathBuf,
    pub preview: PathBuf,
    pub manifest: PathBuf,
}

#[derive(Clone, Copy)]
struct FeaturedFlight<'a> {
    image_number: &'a str,
    provider: Option<&'a str>,
    provider_flight_id: Option<&'a str>,
    date: &'a str,
    date_display: &'a str,
    flight_number: &'a str,
    airline: &'a str,
    callsign: &'a str,
    registration: &'a str,
    aircraft_type: &'a str,
    status: &'a str,
    origin: &'a str,
    destination: &'a str,
    origin_display: &'a str,
    destination_display: &'a str,
    origin_code: &'a str,
    destination_code: &'a str,
    scheduled_departure: &'a str,
    actual_departure: &'a str,
    scheduled_arrival: &'a str,
    actual_arrival: &'a str,
    duration: &'a str,
    arrival_delay: &'a str,
    cruise_altitude: &'a str,
    max_ground_speed: &'a str,
    direct_distance: &'a str,
    track_distance: &'a str,
    extra_distance: &'a str,
    deviation: &'a str,
    reason_primary: &'a str,
    reason_secondary: &'a str,
}

const FIXTURE_FLIGHT: FeaturedFlight<'static> = FeaturedFlight {
    image_number: "000",
    provider: None,
    provider_flight_id: None,
    date: "2026-10-29",
    date_display: "29 OCT 2026",
    flight_number: "DF001",
    airline: "DEMO CARGO",
    callsign: "EXAMPLE1",
    registration: "N747EX",
    aircraft_type: "BOEING 747-8F",
    status: "ARRIVED",
    origin: "ANCHORAGE (ANC)",
    destination: "FRANKFURT (FRA)",
    origin_display: "ANCHORAGE",
    destination_display: "FRANKFURT",
    origin_code: "ANC",
    destination_code: "FRA",
    scheduled_departure: "07:00",
    actual_departure: "07:15",
    scheduled_arrival: "15:35",
    actual_arrival: "16:02",
    duration: "8H 47M",
    arrival_delay: "+27 MIN",
    cruise_altitude: "40,000 FT",
    max_ground_speed: "522 KT",
    direct_distance: "4,055 NM",
    track_distance: "4,210 NM",
    extra_distance: "+155 NM",
    deviation: "+3.8%",
    reason_primary: "SELECTED FOR RARE AIRCRAFT AND ROUTE.",
    reason_secondary: "TRACK 3.8% ABOVE DIRECT DISTANCE.",
};

#[derive(Serialize)]
struct Manifest<'a> {
    schema_version: u8,
    date: &'a str,
    flight: ManifestFlight<'a>,
    display: ManifestDisplay<'a>,
    artifacts: ManifestArtifacts,
    #[serde(skip_serializing_if = "Option::is_none")]
    selection: Option<&'a ScoreBreakdown>,
    fixture: bool,
}

#[derive(Serialize)]
struct ManifestFlight<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_flight_id: Option<&'a str>,
    flight_number: &'a str,
    airline: &'a str,
    callsign: &'a str,
    registration: &'a str,
    aircraft_type: &'a str,
    status: &'a str,
    origin: &'a str,
    destination: &'a str,
    scheduled_departure: &'a str,
    actual_departure: &'a str,
    scheduled_arrival: &'a str,
    actual_arrival: &'a str,
    duration: &'a str,
    arrival_delay: &'a str,
    cruise_altitude: &'a str,
    max_ground_speed: &'a str,
    direct_distance: &'a str,
    track_distance: &'a str,
    extra_distance: &'a str,
    deviation: &'a str,
    reason_primary: &'a str,
    reason_secondary: &'a str,
}

#[derive(Serialize)]
struct ManifestDisplay<'a> {
    image_number: &'a str,
    /// Native controller dimensions used by the binary framebuffer.
    width: u32,
    height: u32,
    /// Human-readable preview dimensions before controller rotation.
    logical_width: u32,
    logical_height: u32,
    orientation: &'static str,
    rotation_degrees: u16,
    encoding: &'static str,
    black_bit: u8,
    binary_size: usize,
}

#[derive(Serialize)]
struct ManifestArtifacts {
    binary: &'static str,
    binary_sha256: String,
    preview: &'static str,
}

pub fn render_fixture(
    output_directory: &Path,
    profile: DisplayProfile,
) -> Result<ArtifactPaths, ArtifactError> {
    render_artifacts(output_directory, profile, FIXTURE_FLIGHT, None, true)
}

pub fn render_selected_flight(
    output_directory: &Path,
    profile: DisplayProfile,
    selected: &SelectedFlight,
    image_number: &str,
) -> Result<ArtifactPaths, ArtifactError> {
    let origin = format!(
        "{} ({})",
        selected.flight.origin.name, selected.flight.origin.code
    );
    let destination = format!(
        "{} ({})",
        selected.flight.destination.name, selected.flight.destination.code
    );
    let origin_display = selected.flight.origin.name.clone();
    let destination_display = selected.flight.destination.name.clone();
    let date_display = display_date(&selected.flight.date);
    let identification = &selected.flight.identification;
    let flight_number = value_or_unknown(identification.flight_number.as_deref());
    let airline = value_or_unknown(identification.airline.as_deref());
    let registration = value_or_unknown(identification.registration.as_deref());
    let status = value_or_unknown(identification.status.as_deref());
    let timings = &selected.flight.timings;
    let scheduled_departure = value_or_unknown(timings.scheduled_departure_utc.as_deref());
    let actual_departure = value_or_unknown(timings.actual_departure_utc.as_deref());
    let scheduled_arrival = value_or_unknown(timings.scheduled_arrival_utc.as_deref());
    let actual_arrival = value_or_unknown(timings.actual_arrival_utc.as_deref());
    let duration = selected
        .flight
        .timings
        .duration_minutes
        .map_or_else(|| "UNKNOWN".to_owned(), format_duration);
    let arrival_delay = timings
        .arrival_delay_minutes
        .map_or_else(|| "UNKNOWN".to_owned(), |delay| format_signed(delay, "MIN"));
    let performance = &selected.flight.performance;
    let cruise_altitude = performance.cruise_altitude_ft.map_or_else(
        || "UNKNOWN".to_owned(),
        |value| format!("{} FT", format_number(value)),
    );
    let max_ground_speed = performance
        .max_ground_speed_knots
        .map_or_else(|| "UNKNOWN".to_owned(), |value| format!("{value} KT"));
    let direct_distance = performance.direct_distance_nm.map_or_else(
        || "UNKNOWN".to_owned(),
        |value| format!("{} NM", format_number(value)),
    );
    let track_distance = performance.track_distance_nm.map_or_else(
        || "UNKNOWN".to_owned(),
        |value| format!("{} NM", format_number(value)),
    );
    let (extra_distance, deviation, reason_secondary) = performance_difference(performance);
    let reason_primary = compact_selection_reason(&selected.score);
    let flight = FeaturedFlight {
        image_number,
        provider: Some(&selected.flight.provider),
        provider_flight_id: Some(&selected.flight.provider_flight_id),
        date: &selected.flight.date,
        date_display: &date_display,
        flight_number: &flight_number,
        airline: &airline,
        callsign: &selected.flight.callsign,
        registration: &registration,
        aircraft_type: &selected.flight.aircraft_type,
        status: &status,
        origin: &origin,
        destination: &destination,
        origin_display: &origin_display,
        destination_display: &destination_display,
        origin_code: &selected.flight.origin.code,
        destination_code: &selected.flight.destination.code,
        scheduled_departure: &scheduled_departure,
        actual_departure: &actual_departure,
        scheduled_arrival: &scheduled_arrival,
        actual_arrival: &actual_arrival,
        duration: &duration,
        arrival_delay: &arrival_delay,
        cruise_altitude: &cruise_altitude,
        max_ground_speed: &max_ground_speed,
        direct_distance: &direct_distance,
        track_distance: &track_distance,
        extra_distance: &extra_distance,
        deviation: &deviation,
        reason_primary: &reason_primary,
        reason_secondary: &reason_secondary,
    };
    render_artifacts(
        output_directory,
        profile,
        flight,
        Some(&selected.score),
        true,
    )
}

fn format_duration(minutes: u32) -> String {
    format!("{}H {:02}M", minutes / 60, minutes % 60)
}

fn value_or_unknown(value: Option<&str>) -> String {
    value.unwrap_or("UNKNOWN").to_owned()
}

fn format_signed(value: i32, unit: &str) -> String {
    format!("{value:+} {unit}")
}

fn format_number(value: u32) -> String {
    let digits = value.to_string();
    let mut output = String::new();
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            output.push(',');
        }
        output.push(character);
    }
    output
}

fn display_date(date: &str) -> String {
    let mut parts = date.split('-');
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return date.to_owned();
    };
    let month = match month {
        "01" => "JAN",
        "02" => "FEB",
        "03" => "MAR",
        "04" => "APR",
        "05" => "MAY",
        "06" => "JUN",
        "07" => "JUL",
        "08" => "AUG",
        "09" => "SEP",
        "10" => "OCT",
        "11" => "NOV",
        "12" => "DEC",
        _ => return date.to_owned(),
    };
    format!("{day} {month} {year}")
}

fn performance_difference(
    performance: &crate::domain::FlightPerformance,
) -> (String, String, String) {
    let (Some(direct), Some(track)) = (
        performance.direct_distance_nm,
        performance.track_distance_nm,
    ) else {
        return (
            "UNKNOWN".into(),
            "UNKNOWN".into(),
            "TRACK COMPARISON UNAVAILABLE.".into(),
        );
    };
    let extra = track as i64 - direct as i64;
    let percentage = if direct == 0 {
        0.0
    } else {
        extra as f64 * 100.0 / f64::from(direct)
    };
    (
        format!("{extra:+} NM"),
        format!("{percentage:+.1}%"),
        format!("TRACK {percentage:.1}% ABOVE DIRECT DISTANCE."),
    )
}

fn compact_selection_reason(score: &ScoreBreakdown) -> String {
    let mut components = [
        (score.aircraft_rarity, "RARE AIRCRAFT"),
        (score.route_rarity, "RARE ROUTE"),
        (score.geographic_interest, "GEOGRAPHY"),
        (score.distance, "DISTANCE"),
        (score.data_quality, "DATA QUALITY"),
    ];
    components.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(right.1)));
    if components[0].1 == "RARE AIRCRAFT" && components[1].1 == "RARE ROUTE" {
        "SELECTED: RARE AIRCRAFT AND ROUTE.".into()
    } else {
        format!("SELECTED: {} + {}.", components[0].1, components[1].1)
    }
}

fn truncate_text(text: &str, max_characters: usize) -> String {
    let characters: Vec<_> = text.chars().collect();
    if characters.len() <= max_characters {
        return text.to_owned();
    }
    characters[..max_characters].iter().collect()
}

fn wrap_text(text: &str, max_characters: usize, max_lines: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let proposed_length =
            current.chars().count() + usize::from(!current.is_empty()) + word.chars().count();
        if proposed_length > max_characters && !current.is_empty() {
            lines.push(current);
            current = String::new();
            if lines.len() == max_lines {
                break;
            }
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() && lines.len() < max_lines {
        lines.push(truncate_text(&current, max_characters));
    }
    lines
}

fn render_artifacts<'a>(
    output_directory: &Path,
    profile: DisplayProfile,
    flight: FeaturedFlight<'a>,
    selection: Option<&'a ScoreBreakdown>,
    fixture: bool,
) -> Result<ArtifactPaths, ArtifactError> {
    profile.validate()?;
    fs::create_dir_all(output_directory)?;

    let (logical_width, logical_height) = profile.logical_dimensions();
    let logical_frame = render_flight(logical_width, logical_height, flight);
    let controller_frame = logical_frame.rotate(profile.rotation_degrees);
    debug_assert_eq!(
        (controller_frame.width, controller_frame.height),
        (profile.width, profile.height)
    );
    let binary = controller_frame.pack_1bpp(profile.black_bit);
    let binary_sha256 = sha256_hex(&binary);

    let paths = ArtifactPaths {
        binary: output_directory.join("display.bin"),
        preview: output_directory.join("preview.png"),
        manifest: output_directory.join("flight.json"),
    };

    fs::write(&paths.binary, &binary)?;
    write_png(&paths.preview, &logical_frame)?;

    let manifest = Manifest {
        schema_version: 1,
        date: flight.date,
        flight: ManifestFlight {
            provider: flight.provider,
            provider_flight_id: flight.provider_flight_id,
            flight_number: flight.flight_number,
            airline: flight.airline,
            callsign: flight.callsign,
            registration: flight.registration,
            aircraft_type: flight.aircraft_type,
            status: flight.status,
            origin: flight.origin,
            destination: flight.destination,
            scheduled_departure: flight.scheduled_departure,
            actual_departure: flight.actual_departure,
            scheduled_arrival: flight.scheduled_arrival,
            actual_arrival: flight.actual_arrival,
            duration: flight.duration,
            arrival_delay: flight.arrival_delay,
            cruise_altitude: flight.cruise_altitude,
            max_ground_speed: flight.max_ground_speed,
            direct_distance: flight.direct_distance,
            track_distance: flight.track_distance,
            extra_distance: flight.extra_distance,
            deviation: flight.deviation,
            reason_primary: flight.reason_primary,
            reason_secondary: flight.reason_secondary,
        },
        display: ManifestDisplay {
            image_number: flight.image_number,
            width: profile.width,
            height: profile.height,
            logical_width,
            logical_height,
            orientation: if logical_height > logical_width {
                "portrait"
            } else {
                "landscape"
            },
            rotation_degrees: profile.rotation_degrees,
            encoding: "raw-1bpp-row-major-msb-first",
            black_bit: profile.black_bit,
            binary_size: binary.len(),
        },
        artifacts: ManifestArtifacts {
            binary: "display.bin",
            binary_sha256,
            preview: "preview.png",
        },
        selection,
        fixture,
    };
    let json = serde_json::to_vec_pretty(&manifest)?;
    fs::write(&paths.manifest, json)?;

    Ok(paths)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn write_png(path: &Path, framebuffer: &FrameBuffer) -> Result<(), ArtifactError> {
    let file = File::create(path)?;
    let mut encoder =
        png::Encoder::new(BufWriter::new(file), framebuffer.width, framebuffer.height);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&framebuffer.as_grayscale())?;
    writer.finish()?;
    Ok(())
}

#[derive(Clone)]
struct FrameBuffer {
    width: u32,
    height: u32,
    /// `true` is black and `false` is white.
    pixels: Vec<bool>,
}

impl FrameBuffer {
    fn white(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![false; (width * height) as usize],
        }
    }

    fn set_black(&mut self, x: i32, y: i32) {
        if x >= 0 && y >= 0 && x < self.width as i32 && y < self.height as i32 {
            self.pixels[(y as u32 * self.width + x as u32) as usize] = true;
        }
    }

    fn horizontal_line(&mut self, x0: i32, x1: i32, y: i32) {
        for x in x0.min(x1)..=x0.max(x1) {
            self.set_black(x, y);
        }
    }

    fn vertical_line(&mut self, x: i32, y0: i32, y1: i32) {
        for y in y0.min(y1)..=y0.max(y1) {
            self.set_black(x, y);
        }
    }

    fn rectangle(&mut self, x: i32, y: i32, width: i32, height: i32) {
        self.horizontal_line(x, x + width - 1, y);
        self.horizontal_line(x, x + width - 1, y + height - 1);
        self.vertical_line(x, y, y + height - 1);
        self.vertical_line(x + width - 1, y, y + height - 1);
    }

    fn text(&mut self, x: i32, y: i32, text: &str, scale: i32) {
        let mut cursor_x = x;
        for character in text.chars() {
            let glyph = glyph(character);
            for (column, bits) in glyph.iter().enumerate() {
                for row in 0..7 {
                    if bits & (1 << row) != 0 {
                        for offset_y in 0..scale {
                            for offset_x in 0..scale {
                                self.set_black(
                                    cursor_x + column as i32 * scale + offset_x,
                                    y + row * scale + offset_y,
                                );
                            }
                        }
                    }
                }
            }
            cursor_x += 6 * scale;
        }
    }

    fn pack_1bpp(&self, black_bit: u8) -> Vec<u8> {
        let mut output = Vec::with_capacity((self.width * self.height / 8) as usize);
        for row in self.pixels.chunks_exact(self.width as usize) {
            for pixels in row.chunks_exact(8) {
                let mut byte = 0_u8;
                for (position, is_black) in pixels.iter().enumerate() {
                    let bit = if *is_black { black_bit } else { 1 - black_bit };
                    byte |= bit << (7 - position);
                }
                output.push(byte);
            }
        }
        output
    }

    fn as_grayscale(&self) -> Vec<u8> {
        self.pixels
            .iter()
            .map(|is_black| if *is_black { 0 } else { 255 })
            .collect()
    }

    fn rotate(&self, degrees: u16) -> Self {
        let (output_width, output_height) = match degrees {
            90 | 270 => (self.height, self.width),
            _ => (self.width, self.height),
        };
        let mut output = Self::white(output_width, output_height);

        for source_y in 0..self.height {
            for source_x in 0..self.width {
                let source_index = (source_y * self.width + source_x) as usize;
                if !self.pixels[source_index] {
                    continue;
                }
                let (target_x, target_y) = match degrees {
                    0 => (source_x, source_y),
                    90 => (self.height - 1 - source_y, source_x),
                    180 => (self.width - 1 - source_x, self.height - 1 - source_y),
                    270 => (source_y, self.width - 1 - source_x),
                    _ => unreachable!("rotation is validated by DisplayProfile"),
                };
                output.set_black(target_x as i32, target_y as i32);
            }
        }
        output
    }
}

fn render_flight(width: u32, height: u32, flight: FeaturedFlight) -> FrameBuffer {
    let mut frame = FrameBuffer::white(width, height);
    let width = width as i32;
    let height = height as i32;
    let margin = (width / 40).max(10);

    if height > width {
        render_portrait(&mut frame, width, height, margin, flight);
    } else {
        render_landscape(&mut frame, width, height, margin, flight);
    }
    frame
}

fn render_portrait(
    frame: &mut FrameBuffer,
    _width: i32,
    _height: i32,
    _margin: i32,
    flight: FeaturedFlight,
) {
    const LEFT: i32 = 24;
    const RIGHT: i32 = 252;

    frame.text(LEFT, 22, "DAILY FLIGHT", 2);
    frame.text(
        344,
        27,
        &format!("{} / {}", flight.date_display, flight.image_number),
        1,
    );
    divider_at(frame, 57);

    frame.text(LEFT, 82, flight.origin_code, 5);
    frame.text(224, 105, "TO", 2);
    frame.text(354, 82, flight.destination_code, 5);
    draw_wrapped_text(frame, LEFT, 139, flight.origin_display, 1, 32, 2);
    draw_wrapped_text(frame, RIGHT, 139, flight.destination_display, 1, 32, 2);
    divider_at(frame, 164);

    frame.text(LEFT, 181, "01 / IDENTIFICATION", 1);
    field(frame, LEFT, 205, "FLIGHT", flight.flight_number);
    field(frame, RIGHT, 205, "AIRLINE", flight.airline);
    field(frame, LEFT, 249, "CALLSIGN", flight.callsign);
    field(frame, RIGHT, 249, "REGISTRATION", flight.registration);
    field(frame, LEFT, 293, "AIRCRAFT", flight.aircraft_type);
    field(frame, RIGHT, 293, "STATUS", flight.status);
    divider_at(frame, 337);

    frame.text(LEFT, 353, "02 / TIMINGS (UTC)", 1);
    field(
        frame,
        LEFT,
        375,
        "SCHEDULED DEP",
        flight.scheduled_departure,
    );
    field(frame, RIGHT, 375, "ACTUAL DEP", flight.actual_departure);
    field(frame, LEFT, 419, "SCHEDULED ARR", flight.scheduled_arrival);
    field(frame, RIGHT, 419, "ACTUAL ARR", flight.actual_arrival);
    field(frame, LEFT, 463, "FLIGHT TIME", flight.duration);
    field(frame, RIGHT, 463, "ARRIVAL DELAY", flight.arrival_delay);
    divider_at(frame, 507);

    frame.text(LEFT, 523, "03 / PERFORMANCE", 1);
    field(frame, LEFT, 545, "CRUISE ALTITUDE", flight.cruise_altitude);
    field(
        frame,
        RIGHT,
        545,
        "MAX GROUND SPEED",
        flight.max_ground_speed,
    );
    field(frame, LEFT, 589, "DIRECT DISTANCE", flight.direct_distance);
    field(frame, RIGHT, 589, "TRACK DISTANCE", flight.track_distance);
    field(frame, LEFT, 633, "EXTRA DISTANCE", flight.extra_distance);
    field(frame, RIGHT, 633, "DEVIATION", flight.deviation);
    divider_at(frame, 676);

    frame.text(LEFT, 694, "04 / WHY THIS FLIGHT", 1);
    frame.text(LEFT, 715, flight.reason_primary, 2);
    frame.text(LEFT, 741, flight.reason_secondary, 2);
    divider_at(frame, 773);
    frame.text(LEFT, 783, "ILLUSTRATIVE DATA / NOT LIVE", 1);
}

fn divider_at(frame: &mut FrameBuffer, y: i32) {
    frame.horizontal_line(24, 456, y);
}

fn field(frame: &mut FrameBuffer, x: i32, y: i32, label: &str, value: &str) {
    frame.text(x, y, label, 1);
    frame.text(x, y + 14, value, 2);
}

fn render_landscape(
    frame: &mut FrameBuffer,
    width: i32,
    height: i32,
    margin: i32,
    flight: FeaturedFlight,
) {
    frame.rectangle(margin, margin, width - margin * 2, height - margin * 2);
    let left = margin + 16;
    let right = width / 2 + 12;
    frame.text(left, margin + 12, "DAILY FLIGHT", 3);
    frame.text(width - margin - 280, margin + 16, flight.date_display, 2);
    frame.text(
        width - margin - 82,
        margin + 12,
        &format!("#{}", flight.image_number),
        3,
    );
    frame.horizontal_line(margin, width - margin - 1, margin + 48);

    frame.text(left, margin + 70, "FROM", 2);
    frame.text(left + 86, margin + 60, flight.origin_code, 5);
    frame.text(left, margin + 112, flight.origin_display, 2);
    frame.text(left, margin + 152, "TO", 2);
    frame.text(left + 86, margin + 142, flight.destination_code, 5);
    frame.text(left, margin + 194, flight.destination_display, 2);

    frame.vertical_line(width / 2 - 10, margin + 60, height - margin - 16);
    frame.text(right, margin + 70, "1 IDENTIFICATION", 2);
    frame.text(
        right,
        margin + 96,
        &format!("{} / {}", flight.callsign, flight.aircraft_type),
        2,
    );
    frame.text(right, margin + 136, "2 TIMINGS", 2);
    frame.text(
        right,
        margin + 162,
        &format!("{} - {}", flight.actual_departure, flight.actual_arrival),
        2,
    );
    frame.text(
        right,
        margin + 184,
        &format!("DURATION {}", flight.duration),
        2,
    );
    frame.text(right, margin + 224, "3 PERFORMANCE", 2);
    frame.text(
        right,
        margin + 250,
        &format!("DISTANCE {}", flight.direct_distance),
        2,
    );
    frame.text(
        right,
        margin + 272,
        &format!("SPEED {}", flight.max_ground_speed),
        2,
    );
    frame.text(
        right,
        margin + 294,
        &format!("ALTITUDE {}", flight.cruise_altitude),
        2,
    );
    frame.text(right, margin + 334, "4 WHY THIS FLIGHT", 2);
    draw_wrapped_text(frame, right, margin + 360, flight.reason_primary, 2, 29, 3);
}

fn draw_wrapped_text(
    frame: &mut FrameBuffer,
    x: i32,
    y: i32,
    text: &str,
    scale: i32,
    max_characters: usize,
    max_lines: usize,
) {
    for (line_index, line) in wrap_text(text, max_characters, max_lines)
        .iter()
        .enumerate()
    {
        frame.text(x, y + line_index as i32 * scale * 9, line, scale);
    }
}

/// Five columns, seven rows, least-significant bit at the top.
fn glyph(character: char) -> [u8; 5] {
    match character.to_ascii_uppercase() {
        'A' => [0x7e, 0x09, 0x09, 0x09, 0x7e],
        'B' => [0x7f, 0x49, 0x49, 0x49, 0x36],
        'C' => [0x3e, 0x41, 0x41, 0x41, 0x22],
        'D' => [0x7f, 0x41, 0x41, 0x22, 0x1c],
        'E' => [0x7f, 0x49, 0x49, 0x49, 0x41],
        'F' => [0x7f, 0x09, 0x09, 0x09, 0x01],
        'G' => [0x3e, 0x41, 0x49, 0x49, 0x7a],
        'H' => [0x7f, 0x08, 0x08, 0x08, 0x7f],
        'I' => [0x00, 0x41, 0x7f, 0x41, 0x00],
        'J' => [0x20, 0x40, 0x41, 0x3f, 0x01],
        'K' => [0x7f, 0x08, 0x14, 0x22, 0x41],
        'L' => [0x7f, 0x40, 0x40, 0x40, 0x40],
        'M' => [0x7f, 0x02, 0x0c, 0x02, 0x7f],
        'N' => [0x7f, 0x04, 0x08, 0x10, 0x7f],
        'O' => [0x3e, 0x41, 0x41, 0x41, 0x3e],
        'P' => [0x7f, 0x09, 0x09, 0x09, 0x06],
        'Q' => [0x3e, 0x41, 0x51, 0x21, 0x5e],
        'R' => [0x7f, 0x09, 0x19, 0x29, 0x46],
        'S' => [0x46, 0x49, 0x49, 0x49, 0x31],
        'T' => [0x01, 0x01, 0x7f, 0x01, 0x01],
        'U' => [0x3f, 0x40, 0x40, 0x40, 0x3f],
        'V' => [0x1f, 0x20, 0x40, 0x20, 0x1f],
        'W' => [0x7f, 0x20, 0x18, 0x20, 0x7f],
        'X' => [0x63, 0x14, 0x08, 0x14, 0x63],
        'Y' => [0x03, 0x04, 0x78, 0x04, 0x03],
        'Z' => [0x61, 0x51, 0x49, 0x45, 0x43],
        '0' => [0x3e, 0x51, 0x49, 0x45, 0x3e],
        '1' => [0x00, 0x42, 0x7f, 0x40, 0x00],
        '2' => [0x42, 0x61, 0x51, 0x49, 0x46],
        '3' => [0x21, 0x41, 0x45, 0x4b, 0x31],
        '4' => [0x18, 0x14, 0x12, 0x7f, 0x10],
        '5' => [0x27, 0x45, 0x45, 0x45, 0x39],
        '6' => [0x3c, 0x4a, 0x49, 0x49, 0x30],
        '7' => [0x01, 0x71, 0x09, 0x05, 0x03],
        '8' => [0x36, 0x49, 0x49, 0x49, 0x36],
        '9' => [0x06, 0x49, 0x49, 0x29, 0x1e],
        '-' => [0x08, 0x08, 0x08, 0x08, 0x08],
        '(' => [0x00, 0x1c, 0x22, 0x41, 0x00],
        ')' => [0x00, 0x41, 0x22, 0x1c, 0x00],
        '.' => [0x00, 0x60, 0x60, 0x00, 0x00],
        ',' => [0x00, 0x40, 0x20, 0x00, 0x00],
        ':' => [0x00, 0x36, 0x36, 0x00, 0x00],
        '/' => [0x20, 0x10, 0x08, 0x04, 0x02],
        '#' => [0x14, 0x7f, 0x14, 0x7f, 0x14],
        '+' => [0x08, 0x08, 0x3e, 0x08, 0x08],
        '%' => [0x63, 0x13, 0x08, 0x64, 0x63],
        ' ' => [0; 5],
        _ => [0x02, 0x01, 0x51, 0x09, 0x06],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_pixels_are_msb_first_and_use_configured_polarity() {
        let frame = FrameBuffer {
            width: 8,
            height: 1,
            pixels: vec![true, false, true, false, true, false, true, false],
        };

        assert_eq!(frame.pack_1bpp(0), vec![0b0101_0101]);
        assert_eq!(frame.pack_1bpp(1), vec![0b1010_1010]);
    }

    #[test]
    fn binary_round_trip_matches_the_framebuffer() {
        let profile = DisplayProfile::default();
        let (logical_width, logical_height) = profile.logical_dimensions();
        let logical_frame = render_flight(logical_width, logical_height, FIXTURE_FLIGHT);
        let controller_frame = logical_frame.rotate(profile.rotation_degrees);
        let packed = controller_frame.pack_1bpp(profile.black_bit);

        assert_eq!(packed.len(), (profile.width * profile.height / 8) as usize);
        for (pixel_index, expected_black) in controller_frame.pixels.iter().enumerate() {
            let byte = packed[pixel_index / 8];
            let actual_bit = (byte >> (7 - pixel_index % 8)) & 1;
            let actual_black = actual_bit == profile.black_bit;
            assert_eq!(actual_black, *expected_black, "pixel {pixel_index}");
        }
    }

    #[test]
    fn profile_rejects_non_byte_aligned_width() {
        let result = DisplayProfile {
            width: 801,
            ..DisplayProfile::default()
        }
        .validate();

        assert!(matches!(result, Err(ArtifactError::InvalidProfile(_))));
    }

    #[test]
    fn portrait_frame_rotates_to_native_controller_dimensions() {
        let profile = DisplayProfile::default();
        let (logical_width, logical_height) = profile.logical_dimensions();
        let logical_frame = FrameBuffer::white(logical_width, logical_height);
        let controller_frame = logical_frame.rotate(profile.rotation_degrees);

        assert_eq!((logical_frame.width, logical_frame.height), (480, 800));
        assert_eq!(
            (controller_frame.width, controller_frame.height),
            (800, 480)
        );
    }

    #[test]
    fn fixture_output_is_deterministic_and_consistent() {
        let directory =
            std::env::temp_dir().join(format!("flight-a-day-artifact-test-{}", std::process::id()));
        if directory.exists() {
            fs::remove_dir_all(&directory).expect("remove stale test directory");
        }

        let profile = DisplayProfile::default();
        let paths = render_fixture(&directory, profile).expect("render fixture");
        let first_binary = fs::read(&paths.binary).expect("read binary");
        let first_png = fs::read(&paths.preview).expect("read PNG");
        let first_json = fs::read(&paths.manifest).expect("read manifest");

        render_fixture(&directory, profile).expect("render fixture again");
        assert_eq!(fs::read(&paths.binary).unwrap(), first_binary);
        assert_eq!(fs::read(&paths.preview).unwrap(), first_png);
        assert_eq!(fs::read(&paths.manifest).unwrap(), first_json);
        assert_eq!(first_binary.len(), 48_000);

        let manifest: serde_json::Value = serde_json::from_slice(&first_json).unwrap();
        assert_eq!(manifest["display"]["width"], 800);
        assert_eq!(manifest["display"]["height"], 480);
        assert_eq!(manifest["display"]["logical_width"], 480);
        assert_eq!(manifest["display"]["logical_height"], 800);
        assert_eq!(manifest["display"]["orientation"], "portrait");
        assert_eq!(manifest["display"]["rotation_degrees"], 90);
        assert_eq!(manifest["display"]["binary_size"], 48_000);
        assert_eq!(
            manifest["artifacts"]["binary_sha256"],
            sha256_hex(&first_binary)
        );

        let decoder = png::Decoder::new(std::io::Cursor::new(first_png));
        let mut reader = decoder.read_info().expect("decode PNG header");
        let (logical_width, logical_height) = profile.logical_dimensions();
        assert_eq!(reader.info().width, logical_width);
        assert_eq!(reader.info().height, logical_height);
        let mut decoded_png = vec![0; reader.output_buffer_size().expect("PNG buffer size")];
        let output = reader
            .next_frame(&mut decoded_png)
            .expect("decode PNG pixels");
        decoded_png.truncate(output.buffer_size());
        let expected_frame = render_flight(logical_width, logical_height, FIXTURE_FLIGHT);
        assert_eq!(decoded_png, expected_frame.as_grayscale());

        fs::remove_dir_all(directory).expect("remove test directory");
    }
}
