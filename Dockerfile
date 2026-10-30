FROM rust:1-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
COPY fixtures/flights.json ./fixtures/flights.json
RUN cargo build --locked --release

FROM debian:bookworm-slim
RUN groupadd --gid 10001 flight && useradd --uid 10001 --gid flight --no-create-home flight \
    && mkdir -p /data /app && chown flight:flight /data /app
COPY --from=builder /build/target/release/Flight-A-Day /usr/local/bin/flight-a-day
WORKDIR /app
ENV FLIGHT_DATA_DIR=/data FLIGHT_SOURCE=synthetic FLIGHT_RUN_TIME=04:00
USER 10001:10001
HEALTHCHECK --interval=60s --timeout=10s --start-period=120s --retries=3 \
    CMD ["flight-a-day", "health"]
ENTRYPOINT ["flight-a-day"]
CMD ["serve"]
