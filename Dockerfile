FROM ekidd/rust-musl-builder:latest AS builder

WORKDIR /app

ADD --chown=rust:rust . /app/

RUN cargo build --release

FROM alpine:latest

RUN apk --no-cache add ca-certificates
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/panamax /usr/local/bin

ENTRYPOINT [ "/usr/local/bin/panamax" ]
CMD ["--help"]