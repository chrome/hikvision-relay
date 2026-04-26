FROM rust:1-bookworm AS builder

WORKDIR /app

# Cache dependency build artifacts first for faster rebuilds.
COPY Cargo.toml Cargo.lock build.rs wrapper.h ./
COPY src ./src

RUN cargo build --release

FROM debian:bookworm-slim AS runtime

ENV APP_HOME=/app
ENV SDK_ROOT=/opt/hikvision/sdk

WORKDIR ${APP_HOME}

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/hikvision-relay /usr/local/bin/hikvision-relay

# Copy Hikvision SDK from local repository folder.
# Make sure ./sdk contains the Linux SDK package root.
COPY sdk/ ${SDK_ROOT}/

COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh

RUN chmod +x /usr/local/bin/entrypoint.sh

ENV HIKVISION_SDK_PATH=${SDK_ROOT}
ENV LD_LIBRARY_PATH=${SDK_ROOT}/lib

EXPOSE 8554

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
