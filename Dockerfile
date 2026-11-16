FROM rust:1-slim AS build
# git lets the build record which commit it came from (shown on the page).
RUN apt-get update && apt-get install -y --no-install-recommends git && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --release --locked

FROM debian:stable-slim
COPY --from=build /src/target/release/tcpdump-dev /usr/local/bin/tcpdump-dev
# Mount your own config directory over /config to edit the file live.
COPY tcpdump.toml /config/tcpdump.toml
ENV LISTEN=0.0.0.0:8080
EXPOSE 8080
CMD ["tcpdump-dev", "/config/tcpdump.toml"]
