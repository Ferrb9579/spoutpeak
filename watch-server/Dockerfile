# Spoutpeak watch-together server.
#   docker build -t spoutpeak-watch ./watch-server
#   docker run -p 7878:7878 spoutpeak-watch
# Serve it behind HTTPS (a reverse proxy or your platform's TLS) so the app
# connects with wss://<host>/ws.

FROM rust:1-slim-bookworm AS build
WORKDIR /src
# Dependencies first, so code changes don't rebuild them.
COPY Cargo.toml Cargo.lock* ./
RUN mkdir src && echo "fn main() {}" > src/main.rs \
    && cargo build --release && rm -rf src target/release/deps/watch_server*
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
RUN useradd --system --no-create-home watch
COPY --from=build /src/target/release/watch-server /usr/local/bin/watch-server
USER watch
ENV PORT=7878
EXPOSE 7878
# Health: GET /health answers "ok".
CMD ["watch-server"]
