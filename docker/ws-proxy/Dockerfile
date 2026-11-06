FROM rust:1.96-alpine AS build
RUN apk add --no-cache build-base cmake perl linux-headers pkgconf binutils
WORKDIR /build
ENV CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=0
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
RUN cargo build --locked --release -p control-api -p ws-proxy --bin control_api --bin ws-proxy \
    && for binary in target/release/control_api target/release/ws-proxy; do \
         if readelf -d "$binary" | grep -q '(NEEDED)'; then echo "Dynamic dependency in $binary" >&2; exit 1; fi; \
       done

FROM alpine:3.23
RUN apk add --no-cache ca-certificates curl bash
COPY --from=build /build/target/release/control_api /usr/local/bin/control_api
COPY --from=build /build/target/release/ws-proxy /usr/local/bin/ws-proxy
CMD ["control_api"]
