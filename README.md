# Flight A Day

Rust pipeline selecting a daily flight and rendering a 480×800 portrait preview,
800×480 controller framebuffer (48,000 bytes), and JSON manifest. Current data is
synthetic; real providers and Cloudflare publication will come later.

## Homelab

```sh
docker compose up --build -d
docker compose logs -f flight-a-day
docker compose exec flight-a-day flight-a-day health
```

The container runs as UID/GID 10001 with a read-only root filesystem. The named
`flight-data` volume persists SQLite, heartbeat, daily artifacts, and manual test
images under `/data`. Do not remove this volume if you want to preserve history.
For a bind mount instead, make its host directory writable by UID 10001.

Configuration is through environment variables:

| Variable | Default | Meaning |
| --- | --- | --- |
| `FLIGHT_DATA_DIR` | `data` locally, `/data` in container | Database and service state |
| `FLIGHT_OUTPUT_DIR` | `<data>/artifacts` | Daily and test artifact root |
| `FLIGHT_SOURCE` | `synthetic` | `synthetic` or `fixtures` |
| `FLIGHT_FIXTURE_DIR` | `fixtures/generated` locally | Daily `YYYY-MM-DD.json` input files |
| `FLIGHT_RUN_TIME` | `04:00` | Daily time, always UTC |
| `FLIGHT_START_DATE` | empty | Optional earliest day to backfill on first startup |
| `FLIGHT_POLL_SECONDS` | `60` | Retry/schedule check interval, 1–60 seconds |

At 04:00 UTC the service processes the preceding UTC day. Before 04:00 the most
recent due day is two days ago. On first startup it processes the most recent due
day (or backfills from `FLIGHT_START_DATE`); later restarts resume after the saved
scheduler cursor. Jobs run sequentially, failures retry on the next check, and a
failed day blocks later days until fixed. `FLIGHT_START_DATE` applies only before
the first saved cursor. A missing fixture in fixture mode is reported as an error.
UTC scheduling does not shift with daylight saving time.

`serve` holds a filesystem lock to reject a second scheduler or `run-now` using
the same data directory. SIGTERM/Ctrl-C finishes the current daily job and then
stops; idle shutdown takes at most a second. A forced termination can leave a
database run lease active for up to two hours before retry is allowed.

`health` exits nonzero if the heartbeat is over 180 seconds old, a job has failed,
or the scheduler has not caught up. It measures the scheduler, not manual testing.
Large backfills may be unhealthy until complete. Docker restarts exited processes;
an unhealthy status itself does not trigger a Docker restart.

## Immediate ESP32 testing

Generate a different synthetic flight image every time, without waiting for a day:

```sh
docker compose exec flight-a-day flight-a-day refresh-test
docker compose exec flight-a-day flight-a-day refresh-test
```

Or locally:

```powershell
cargo run -- refresh-test
cargo run -- refresh-test --date 2026-10-29
```

Read `/data/artifacts/test/latest.json` (locally `data/artifacts/test/latest.json`).
It contains the newest image number, relative binary/preview paths and SHA-256.
Each call creates `test/000001/`, `test/000002/`, etc., with all three artifacts;
the pointer changes only after they are complete. Numbering survives restarts.
Calls rotate through the valid candidates for the chosen date, wrapping after 470.
All test data is illustrative. Test images do not change daily selections or the
scheduler cursor. Old test revisions are retained for debugging.

These commands create local files; they do not yet upload or expose an HTTP
endpoint. Copy the files into your existing ESP32 test hosting setup. For example:

```sh
docker compose cp flight-a-day:/data/artifacts/test ./esp32-test
```

Force the normal daily pipeline immediately. Because the running scheduler holds
the daily lock, stop it first:

```sh
docker compose stop flight-a-day
docker compose run --rm flight-a-day run-now
docker compose run --rm flight-a-day run-now --date 2026-10-29
docker compose up -d
```

`run-now` defaults to yesterday, bypasses the schedule, and reruns the deterministic
daily selection. Use `refresh-test` to get a different flight. Backfills do not move
the daily `latest.json` pointer backwards. Manual runs do not advance the scheduler
cursor; a later scheduled pass safely reuses valid artifacts.

One scheduler iteration for development or diagnostics:

```powershell
cargo run -- serve --once
cargo run -- health
cargo run -- status --database data/flight-a-day.db
```

`--once` catches up all currently due days and exits; it returns failure if a job
fails. Its heartbeat expires after exit, as expected for a non-running scheduler.

## Existing fixture workflow

```powershell
cargo run -- generate-test-data
cargo run -- run-fixture --input fixtures/generated/2026-10-01.json --database data/synthetic.db --output out/synthetic
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

See [fixtures/README.md](fixtures/README.md) for the saved dataset's assumptions.
