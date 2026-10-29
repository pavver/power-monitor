# Remote support server and Cloudflare Tunnel in one container; nothing is
# published, the tunnel is the only way in. Build from the repository root,
# since the server shares power-core:
#   docker build -f server/Dockerfile -t svitlo-remote .
# or `docker compose up -d --build` in server/ (see compose.yaml).

FROM rust:1.98.1-slim-bookworm AS build
WORKDIR /src
COPY power-core power-core
COPY server server
RUN cd server && cargo build --release --locked

FROM cloudflare/cloudflared:2026.9.3 AS cloudflared

FROM debian:bookworm-20261005-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --home-dir /data svitlo \
    && mkdir /data && chown svitlo /data
COPY --from=build /src/server/target/release/remote-server /usr/local/bin/
COPY --from=cloudflared /usr/local/bin/cloudflared /usr/local/bin/
COPY server/entrypoint.sh /usr/local/bin/
# Only the tunnel reaches the server.
ENV BIND=127.0.0.1:8080 DATA_DIR=/data
VOLUME /data
USER svitlo
ENTRYPOINT ["entrypoint.sh"]
