#!/bin/sh
set -eu
llama-server -m /models/qwen/Qwen3-4B-Q4_K_M.gguf --host 0.0.0.0 --port 8080 -c 8192 --jinja &
llama-server -m /models/embed/Qwen3-Embedding-0.6B-Q8_0.gguf --host 0.0.0.0 --port 8081 --embeddings &
exec python /opt/speech/server.py
