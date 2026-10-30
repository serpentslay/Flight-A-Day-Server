# Synthetic flight data

Generate repeatable daily fixtures with:

```powershell
cargo run -- generate-test-data
```

Defaults: 30 days starting 2026-10-01, 500 flights per day, written to
`fixtures/generated/YYYY-MM-DD.json`. Generated files are ignored by Git.
The generator implementation is committed so the dataset can be reproduced.

```powershell
cargo run -- generate-test-data --days 60 --flights-per-day 1000
cargo run -- run-fixture --input fixtures/generated/2026-10-01.json --database data/synthetic.db --output out/synthetic
```

All flights, operators, registrations and rarity signals are invented. Airport
coordinates are approximate. Routes are not restricted to real scheduled services
or aircraft range. Distances use the great-circle formula; track distance adds
0–19 percent. UTC times wrap at midnight; full arrival dates are not represented.

Every 50 candidates includes three deliberate rejection cases (same airport/ground
movement, invalid latitude, and an out-of-range score) and one candidate with missing
optional identification, timing and performance fields. At 500 flights per day,
470 should pass the current scoring filters. IDs repeat across dates to exercise
date-aware storage. Keep test databases separate from future real collections.

`flights.json` remains the original small, curated display example.
