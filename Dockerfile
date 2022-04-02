FROM rust:latest AS builder

WORKDIR /app

#ADD --chown=rust:rust . /app/
ADD . /app/

ARG CARGO_BUILD_EXTRA
RUN cargo build --release $CARGO_BUILD_EXTRA

FROM debian:latest

COPY --from=builder /app/target/release/panamax /usr/local/bin

RUN apt update \
  && apt install -y \
    ca-certificates \
    git \
    libssl1.1

ENTRYPOINT [ "/usr/local/bin/panamax" ]
CMD ["--help"]
