ARG RUST_IMAGE=rust:alpine
FROM ${RUST_IMAGE} AS build
RUN apk add --no-cache build-base cmake perl linux-headers pkgconf binutils
WORKDIR /build
ENV CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_BUILD_JOBS=2
COPY . .
RUN cargo build --locked --release -p diamant --bin diamant --no-default-features \
    && dynamic="$(readelf -d target/release/diamant)" \
    && headers="$(readelf -l target/release/diamant)" \
    && case "$dynamic" in *'(NEEDED)'*) exit 1 ;; *) true ;; esac \
    && case "$headers" in *INTERP*) exit 1 ;; *) true ;; esac \
    && target/release/diamant --help >/dev/null

FROM scratch AS release-artifact
COPY --from=build /build/target/release/diamant /

FROM alpine:3.23 AS runtime
RUN apk add --no-cache ca-certificates curl bash
COPY --from=build /build/target/release/diamant /usr/local/bin/diamant
RUN diamant --help >/dev/null
CMD ["diamant"]
