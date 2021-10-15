FROM rust:latest AS builder

WORKDIR /app

ADD . /app/

ARG CARGO_BUILD_EXTRA
RUN cargo build --release $CARGO_BUILD_EXTRA

FROM debian:latest

COPY --from=builder /app/target/release/panamax /usr/local/bin

# chagne to ustc source
RUN sed -i "s/deb.debian.org/mirrors.tuna.tsinghua.edu.cn/g" /etc/apt/sources.list
RUN sed -i "s/security.debian.org/mirrors.tuna.tsinghua.edu.cn/g" /etc/apt/sources.list
RUN apt update && apt install -y apt-transport-https ca-certificates
RUN sed -i "s/http/https/g" /etc/apt/sources.list
RUN apt update && apt install -y libssl1.1 git

ENTRYPOINT [ "/usr/local/bin/panamax" ]
CMD ["--help"]
