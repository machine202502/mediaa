#!/bin/sh
set -eu
export LD_LIBRARY_PATH=/usr/local/bin${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}
IDLE="${MEDIAA_IDLE_SECONDS:-120}"
case "$IDLE" in
  ''|*[!0-9]*) IDLE=120 ;;
esac
if [ "$IDLE" -eq 0 ]; then
  SLEEP=-1
else
  SLEEP=$IDLE
fi
llama-server \
  --host 0.0.0.0 --port 8080 \
  --models-preset /opt/llama/chat.ini \
  --models-max 1 \
  --sleep-idle-seconds "$SLEEP" \
  --jinja &
llama-server \
  --host 0.0.0.0 --port 8081 \
  --models-preset /opt/llama/embed.ini \
  --models-max 1 \
  --sleep-idle-seconds "$SLEEP" \
  --embeddings &
exec python /opt/speech/server.py
