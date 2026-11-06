ARG RUST_IMAGE=rust:alpine
FROM ${RUST_IMAGE} AS sources
RUN apk add --no-cache build-base cmake perl linux-headers pkgconf binutils
WORKDIR /build
ENV CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_BUILD_JOBS=2
COPY Cargo.toml Cargo.lock ./
COPY ws-proxy/Cargo.toml ./ws-proxy/
COPY ws-proxy/src ./ws-proxy/src
COPY control-api ./control-api
COPY suntitle-core ./suntitle-core
COPY diamant ./diamant
COPY diamant-ui ./diamant-ui
COPY oci-artifact/Cargo.toml oci-artifact/README.md ./oci-artifact/
COPY oci-artifact/src ./oci-artifact/src
COPY kubernetes ./kubernetes
COPY specs ./specs
COPY docker/ws-proxy/check-static.sh /usr/local/bin/check-static

FROM sources AS control-api-build
RUN cargo build --locked --release -p control-api --bin control_api \
    && sh /usr/local/bin/check-static target/release/control_api

FROM sources AS ws-proxy-build
RUN cargo build --locked --release -p ws-proxy --bin ws-proxy \
    && sh /usr/local/bin/check-static target/release/ws-proxy

FROM sources AS release
RUN cargo build --locked --release -p control-api -p ws-proxy --bin control_api --bin ws-proxy \
    && sh /usr/local/bin/check-static target/release/control_api target/release/ws-proxy

FROM scratch AS release-artifacts
COPY --from=release /build/target/release/control_api /
COPY --from=release /build/target/release/ws-proxy /

FROM alpine:3.23 AS runtime
RUN apk add --no-cache ca-certificates curl bash

FROM runtime AS control-api
COPY --from=control-api-build /build/target/release/control_api /usr/local/bin/control_api
CMD ["control_api"]

FROM runtime AS ws-proxy
COPY --from=ws-proxy-build /build/target/release/ws-proxy /usr/local/bin/ws-proxy
CMD ["ws-proxy"]
