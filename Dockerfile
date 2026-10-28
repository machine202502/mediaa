FROM python:3.11-slim-bookworm

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl libgomp1 tar \
    && rm -rf /var/lib/apt/lists/*

RUN pip install --no-cache-dir sherpa-onnx soundfile numpy huggingface_hub

ARG LLAMA_TAG=b11946
RUN curl -fsSL -o /tmp/llama.tar.gz \
        "https://github.com/ggml-org/llama.cpp/releases/download/${LLAMA_TAG}/llama-${LLAMA_TAG}-bin-ubuntu-x64.tar.gz" \
    && mkdir -p /tmp/llama \
    && tar -xzf /tmp/llama.tar.gz -C /tmp/llama \
    && find /tmp/llama -type f -name llama-server -exec mv {} /usr/local/bin/llama-server \; \
    && chmod +x /usr/local/bin/llama-server \
    && rm -rf /tmp/llama /tmp/llama.tar.gz

ENV HF_HUB_DISABLE_TELEMETRY=1
RUN mkdir -p /models \
    && hf download Qwen/Qwen3-4B-GGUF Qwen3-4B-Q4_K_M.gguf --local-dir /models/qwen \
    && hf download Qwen/Qwen3-Embedding-0.6B-GGUF Qwen3-Embedding-0.6B-Q8_0.gguf --local-dir /models/embed \
    && hf download csukuangfj/sherpa-onnx-nemo-ctc-punct-giga-am-v3-russian-2025-12-16 --local-dir /models/gigaam \
    && hf download csukuangfj/sherpa-onnx-pyannote-segmentation-3-0 --local-dir /models/segmentation \
    && hf download csukuangfj/speaker-embedding-models 3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx --local-dir /models/voices

COPY docker/speech/server.py /opt/speech/server.py
COPY docker/start.sh /opt/start.sh
RUN chmod +x /opt/start.sh

EXPOSE 8080 8081 8293
ENTRYPOINT ["/opt/start.sh"]
