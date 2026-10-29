#!/bin/sh
# Runs the server and the tunnel. When either stops the container stops, so
# Docker's restart policy brings both back together.
set -eu
: "${ADMIN_PASSWORD:?set ADMIN_PASSWORD (12+ characters)}"
: "${TUNNEL_TOKEN:?set TUNNEL_TOKEN: Cloudflare Zero Trust -> Networks -> Tunnels -> your tunnel}"

remote-server &
server=$!
# cloudflared reads TUNNEL_TOKEN itself, which keeps it out of the process list.
cloudflared tunnel --no-autoupdate run &
tunnel=$!

stop() {
    kill "$server" "$tunnel" 2>/dev/null || true
    wait
}
trap 'stop; exit 0' TERM INT

while kill -0 "$server" 2>/dev/null && kill -0 "$tunnel" 2>/dev/null; do
    sleep 2
done
echo "entrypoint: server or tunnel stopped, stopping the container" >&2
stop
exit 1
