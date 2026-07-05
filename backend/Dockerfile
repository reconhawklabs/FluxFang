# --- Build stage ---
FROM rust:1-slim AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    libpcap-dev \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .
RUN cargo build --release --package fluxfang-api

# --- Runtime stage ---
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    libpcap0.8 \
    iw \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/fluxfang-api /usr/local/bin/fluxfang-api

EXPOSE 8080
CMD ["/usr/local/bin/fluxfang-api"]
