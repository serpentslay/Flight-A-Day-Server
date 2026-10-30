# Flight A Day

Rust pipeline selecting a daily flight and rendering a 480×800 portrait preview,
800×480 controller framebuffer (48,000 bytes), and JSON manifest. Current data is
synthetic. Cloudflare R2 publishing is optional; real flight providers will come later.

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

These commands create local files by default. Add `--publish` or enable publishing
below to upload them to R2. There is no built-in HTTP server. To copy local files
into your existing ESP32 test hosting setup:

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

## Cloudflare R2 publishing

Create an R2 bucket and a bucket-scoped Object Read & Write API token. Configure a
public bucket URL: a custom domain for normal operation or the Cloudflare `r2.dev`
development URL while testing. The S3 endpoint is used privately for uploads; it
is different from the public download URL. No R2 credentials belong on the ESP32.
See [Cloudflare S3 setup](https://developers.cloudflare.com/r2/get-started/s3/)
and [public delivery](https://developers.cloudflare.com/r2/buckets/public-buckets/).

Copy `.env.example` to `.env` for Compose. Put the credential values in host files
`secrets/r2-access-key-id` and `secrets/r2-secret-access-key`; allow container UID
10001 to read them. The directory and `.env` are ignored by Git and Docker build.
Set the account endpoint, bucket and public URL in `.env`, then rebuild/recreate:

```sh
docker compose up --build -d
docker compose exec flight-a-day flight-a-day refresh-test --publish
docker compose exec flight-a-day flight-a-day publication-status
```

Shell environment variables are used for local Cargo commands; Cargo does not load
`.env` automatically. Required R2 settings:

| Variable | Meaning |
| --- | --- |
| `R2_ENDPOINT` | `https://<account-id>.r2.cloudflarestorage.com` |
| `R2_BUCKET` | Bucket name |
| `R2_PUBLIC_BASE_URL` | Public HTTPS bucket/custom-domain base URL |
| `R2_ACCESS_KEY_ID_FILE` | Path to mounted access key file |
| `R2_SECRET_ACCESS_KEY_FILE` | Path to mounted secret key file |
| `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY` | Alternative direct environment credentials |
| `FLIGHT_PUBLISH_ENABLED` | `false` by default; `true` automatically publishes scheduled and manual refreshes |
| `FLIGHT_PUBLISH_BACKEND` | `r2` (default) or `local` for offline testing |

Nonempty `_FILE` settings take precedence over direct environment credentials.
Credentials are read explicitly; the AWS machine credential chain is not used.
HTTPS certificate verification remains enabled. The image includes CA certificates.

Publication commands:

```powershell
cargo run -- refresh-test --publish
cargo run -- publish --test
cargo run -- publish --date 2026-10-29
cargo run -- publication-status
```

`publish` uploads existing files without generating a new selection. With no
arguments it uses the last local daily image. `publish --test` retries the last
allocated test revision; if generation itself failed, run `refresh-test` again.
The daily publish command holds the daily lock, so stop the scheduler first when
running it separately in Compose (`docker compose stop flight-a-day`, then
`docker compose run --rm flight-a-day publish --date 2026-10-29`). Start it again
with `docker compose up -d`. Test publication can run alongside the scheduler.

Objects are published as:

```text
daily/<date>/<image-id>/display.bin
daily/<date>/<image-id>/preview.png
daily/<date>/<image-id>/flight.json
daily/latest.json
test/<revision>/<image-id>/display.bin
test/<revision>/<image-id>/preview.png
test/<revision>/<image-id>/flight.json
test/latest.json
```

Every bundle has a SHA-256 content identifier. Its three objects are uploaded and
verified by remote HEAD length/checksum metadata before `latest.json` changes.
The manifest contains `image_id`, absolute `binary_url`, `preview_url`, and
`manifest_url`, plus display dimensions/encoding, byte length, and binary SHA-256.
The ESP32 compares `image_id`, downloads `binary_url`, verifies 48,000 bytes and
the SHA-256, then passes the native bitmap to the panel driver.

Immutable bundles have `Cache-Control: public, max-age=31536000, immutable`;
`daily/latest.json` and `test/latest.json` have `Cache-Control: no-store, max-age=0`.
On your custom domain, configure a cache bypass rule for both latest manifests;
do not use a rule that overrides these headers to cache the pointers. Device
polls should also avoid caching the latest manifest. The cached bundle paths can
be reused indefinitely because changed content gets a new image ID.

Upload state, remote checksums and publication heads are persisted in SQLite via
`0003_publications.sql`. Failed publication leaves the prior remote pointer in
place when a bundle object fails. Transient transport, HTTP 408/429 and server
errors retry up to three times with bounded delays; authorization errors fail
immediately. Retrying HEAD-verifies completed objects and skips their uploads.
A timeout when writing the final pointer can mean the write succeeded remotely;
retry reconciles this safely. Historical daily/test publication never moves a
newer channel pointer backwards. Remote checksum conflicts fail instead of
overwriting an immutable object.

When automatic publication is enabled, the scheduler advances its cursor only
after rendering and publication succeed; upload failures surface in `health` and
retry on the next scheduler check. Enabling publication after local-only runs also
publishes the saved latest day on startup. `pipeline_runs` describes local render
completion; use `publication-status` for upload state. Use one uploader/data
volume per bucket namespace; the cross-process publication lock is local to that
volume. Test revisions are retained without automatic deletion.

### Offline publication test

No Cloudflare account is required to exercise the publication workflow locally:

```powershell
$env:FLIGHT_DATA_DIR = 'data/publish-demo'
$env:FLIGHT_PUBLISH_BACKEND = 'local'
$env:R2_PUBLIC_BASE_URL = 'https://display.example.com'
cargo run -- refresh-test --publish
cargo run -- publish --test
cargo run -- publication-status
```

Objects appear under `data/publish-demo/published`, with metadata sidecars.
`FLIGHT_LOCAL_PUBLISH_DIR` can override that directory locally. This backend
checks actual file hashes but does not start an HTTPS server; the example URLs
are placeholders. Automated tests also exercise partial failures, retries,
pointer ordering, channel independence, and signed SDK requests against a
loopback mock S3 server with invented credentials.
