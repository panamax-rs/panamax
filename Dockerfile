FROM rust:latest AS builder

WORKDIR /app

ADD . /app/

ARG CARGO_BUILD_EXTRA
RUN cargo build --release $CARGO_BUILD_EXTRA

FROM debian:latest

COPY --from=builder /app/target/release/panamax /usr/local/bin
RUN apt update
RUN apt install -y libssl1.1 ca-certificates git

ENTRYPOINT [ "/usr/local/bin/panamax" ]
CMD ["--help"]
