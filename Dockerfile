FROM ekidd/rust-musl-builder:latest AS builder

WORKDIR /app

ADD --chown=rust:rust . /app/

ARG CARGO_BUILD_EXTRA
RUN cargo build --release $CARGO_BUILD_EXTRA

FROM alpine:latest

RUN apk --no-cache add ca-certificates
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/panamax /usr/local/bin

ENTRYPOINT [ "/usr/local/bin/panamax" ]
CMD ["--help"]