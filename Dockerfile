FROM rust:1.91-bookworm AS chef
RUN cargo install cargo-chef
WORKDIR /build

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
ENV RUST_BACKTRACE=1
ENV SQLX_OFFLINE=true

COPY --from=planner /build/recipe.json recipe.json
# Build dependencies - this is the caching Docker layer!
RUN --mount=type=cache,target=$CARGO_HOME/git \
    --mount=type=cache,target=$CARGO_HOME/registry \
    --mount=type=cache,target=/build/target \
    cargo chef cook --release --recipe-path recipe.json

# Build application
COPY . .
RUN --mount=type=cache,target=$CARGO_HOME/git \
    --mount=type=cache,target=$CARGO_HOME/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release && cp /build/target/release/postgate /build/output

FROM debian:bookworm-slim

RUN apt-get update \
    # Install ca-certificates and wget (used for healthcheck)
    && apt-get install -y ca-certificates wget \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/output /usr/local/bin/postgate

CMD ["/usr/local/bin/postgate"]
