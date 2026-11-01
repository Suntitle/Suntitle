FROM rust:1.96-bookworm AS build
WORKDIR /build
ENV CARGO_PROFILE_DEV_DEBUG=0
COPY ws-proxy/Cargo.toml ws-proxy/Cargo.lock ./ws-proxy/
COPY ws-proxy/src ./ws-proxy/src
COPY oci-artifact/Cargo.toml oci-artifact/README.md ./oci-artifact/
COPY oci-artifact/src ./oci-artifact/src
COPY kubernetes ./kubernetes
COPY specs ./specs
RUN cargo build --locked --manifest-path ws-proxy/Cargo.toml --bin control_api --bin ws-proxy

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /build/ws-proxy/target/debug/control_api /usr/local/bin/control_api
COPY --from=build /build/ws-proxy/target/debug/ws-proxy /usr/local/bin/ws-proxy
CMD ["control_api"]
