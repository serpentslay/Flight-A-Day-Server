PRAGMA foreign_keys = ON;

CREATE TABLE pipeline_runs (
    run_date       TEXT PRIMARY KEY,
    status         TEXT NOT NULL,
    stage          TEXT NOT NULL,
    attempt_count  INTEGER NOT NULL DEFAULT 1,
    started_at     TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    completed_at   TEXT,
    lease_until    TEXT,
    lease_token    TEXT,
    error          TEXT
);

CREATE TABLE flights (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    provider            TEXT NOT NULL,
    provider_flight_id  TEXT NOT NULL,
    flight_date         TEXT NOT NULL,
    payload_json        TEXT NOT NULL,
    updated_at          TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(provider, provider_flight_id, flight_date)
);

CREATE INDEX flights_date_idx ON flights(flight_date);

CREATE TABLE flight_scores (
    flight_id              INTEGER NOT NULL REFERENCES flights(id) ON DELETE CASCADE,
    algorithm_version      TEXT NOT NULL,
    total                  INTEGER NOT NULL,
    aircraft_rarity        INTEGER NOT NULL,
    route_rarity           INTEGER NOT NULL,
    geographic_interest    INTEGER NOT NULL,
    distance               INTEGER NOT NULL,
    data_quality           INTEGER NOT NULL,
    explanation            TEXT NOT NULL,
    scored_at              TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY(flight_id, algorithm_version)
);

CREATE TABLE daily_selections (
    selection_date     TEXT PRIMARY KEY,
    flight_id          INTEGER NOT NULL REFERENCES flights(id),
    algorithm_version  TEXT NOT NULL,
    display_number     TEXT NOT NULL,
    explanation        TEXT NOT NULL,
    selected_at        TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE artifacts (
    selection_date   TEXT NOT NULL REFERENCES daily_selections(selection_date) ON DELETE CASCADE,
    kind             TEXT NOT NULL,
    path             TEXT NOT NULL,
    size_bytes       INTEGER NOT NULL,
    sha256           TEXT NOT NULL,
    renderer_version TEXT NOT NULL,
    input_hash       TEXT NOT NULL,
    created_at       TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY(selection_date, kind)
);
