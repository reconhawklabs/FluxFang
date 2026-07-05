# --- Build stage ---
# Pin to bookworm so the builder's glibc matches the debian:bookworm-slim
# runtime below (a bare `rust:1-slim` tracks Debian testing/trixie, whose newer
# glibc produces a binary the bookworm runtime can't load).
FROM rust:1-slim-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    libpcap-dev \
    libudev-dev \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .
RUN cargo build --release --package fluxfang-api

# --- Runtime stage ---
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    libpcap0.8 \
    libudev1 \
    iw \
    iproute2 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/fluxfang-api /usr/local/bin/fluxfang-api

EXPOSE 8080
CMD ["/usr/local/bin/fluxfang-api"]
