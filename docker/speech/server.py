import gc
import json
import os
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import numpy as np
import sherpa_onnx
import soundfile as sf

HOST = "0.0.0.0"
PORT = 8293
KEY = os.environ.get("MEDIAA_SPEECH_KEY", "mediaa")
RATE = 16000
GIGA = "/models/gigaam"
SEG = "/models/segmentation/model.onnx"
EMB = "/models/voices/3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx"


def find_model(directory):
    for name in ("model.int8.onnx", "model.onnx"):
        path = os.path.join(directory, name)
        if os.path.isfile(path):
            return path
    raise SystemExit(f"no GigaAM onnx in {directory}")


def words_from_tokens(tokens, starts, end):
    found = []
    separated = True
    for index, token in enumerate(tokens):
        at = float(starts[index]) if index < len(starts) else 0.0
        piece = token.lstrip("▁ ")
        new_word = separated or token.startswith("▁") or token.startswith(" ")
        if new_word and found:
            found[-1]["end"] = max(found[-1]["end"], at)
        separated = piece == ""
        if piece == "":
            continue
        if found and not new_word:
            found[-1]["word"] += piece
        else:
            found.append({"word": piece, "start": at, "end": at})
    if found:
        found[-1]["end"] = max(end, found[-1]["start"])
    for word in found:
        word["end"] = min(word["end"], word["start"] + 2.0)
    return found


def load():
    asr = sherpa_onnx.OfflineRecognizer.from_nemo_ctc(
        model=find_model(GIGA),
        tokens=os.path.join(GIGA, "tokens.txt"),
    )
    config = sherpa_onnx.OfflineSpeakerDiarizationConfig(
        segmentation=sherpa_onnx.OfflineSpeakerSegmentationModelConfig(
            pyannote=sherpa_onnx.OfflineSpeakerSegmentationPyannoteModelConfig(model=SEG)
        ),
        embedding=sherpa_onnx.SpeakerEmbeddingExtractorConfig(model=EMB),
        clustering=sherpa_onnx.FastClusteringConfig(num_clusters=-1, threshold=0.5),
    )
    diarizer = sherpa_onnx.OfflineSpeakerDiarization(config)
    extractor = sherpa_onnx.SpeakerEmbeddingExtractor(
        sherpa_onnx.SpeakerEmbeddingExtractorConfig(model=EMB)
    )
    return asr, diarizer, extractor


IDLE = int(os.environ.get("MEDIAA_IDLE_SECONDS", "120") or "0")
_LOCK = threading.Lock()
_MODELS = None
_BUSY = 0
_LAST = 0.0


def borrow():
    global _MODELS, _BUSY, _LAST
    with _LOCK:
        if _MODELS is None:
            _MODELS = load()
        _BUSY += 1
        _LAST = time.time()
        return _MODELS


def release():
    global _BUSY, _LAST
    with _LOCK:
        _BUSY = max(0, _BUSY - 1)
        _LAST = time.time()


def drop_if_idle():
    global _MODELS
    with _LOCK:
        if _MODELS is None or _BUSY or IDLE <= 0:
            return None
        if time.time() - _LAST < IDLE:
            return None
        models = _MODELS
        _MODELS = None
        return models


def reap():
    while True:
        time.sleep(15)
        dropped = drop_if_idle()
        if dropped is not None:
            del dropped
            gc.collect()


threading.Thread(target=reap, daemon=True).start()


def to_mono(samples):
    array = np.asarray(samples, dtype=np.float32)
    if array.ndim > 1:
        array = array.mean(axis=1)
    return array


def resample(samples, source_rate):
    if source_rate == RATE or len(samples) == 0:
        return samples
    count = max(1, int(round(len(samples) * RATE / source_rate)))
    grid = np.linspace(0, len(samples) - 1, count)
    return np.interp(grid, np.arange(len(samples)), samples).astype(np.float32)


def read_audio(path):
    samples, rate = sf.read(path, dtype="float32", always_2d=False)
    return resample(to_mono(samples), int(rate))


def transcribe(asr, samples):
    stream = asr.create_stream()
    stream.accept_waveform(RATE, samples)
    asr.decode_stream(stream)
    result = stream.result
    duration = float(len(samples)) / RATE
    tokens = list(getattr(result, "tokens", []) or [])
    starts = list(getattr(result, "timestamps", []) or [])
    text = (getattr(result, "text", "") or "").strip()
    found = words_from_tokens(tokens, starts, duration)
    if not text and found:
        text = " ".join(word["word"] for word in found)
    return text, found, duration


def transcribe_long(asr, samples):
    total = float(len(samples)) / RATE
    if total <= 100.0:
        return transcribe(asr, samples)
    words = []
    start = 0.0
    while start < total - 0.05:
        end = min(total, start + 100.0)
        low = int(start * RATE)
        high = max(low + 1, int(end * RATE))
        _text, found, _duration = transcribe(asr, samples[low:high])
        cutoff = start if start == 0.0 else start + 2.0
        for word in found:
            word["start"] += start
            word["end"] += start
            if word["start"] >= cutoff:
                words.append(word)
        if end >= total:
            break
        start = end - 2.0
    text = " ".join(word["word"] for word in words)
    return text, words, total


def diarize(diarizer, samples):
    result = diarizer.process(samples)
    ordered = result.sort_by_start_time() if hasattr(result, "sort_by_start_time") else result
    turns = []
    for segment in ordered:
        turns.append(
            {
                "start": float(segment.start),
                "end": float(segment.end),
                "speaker": str(segment.speaker),
            }
        )
    return turns


def speaker_of(word, turns):
    best = None
    overlap = 0.0
    for turn in turns:
        left = max(word["start"], turn["start"])
        right = min(word["end"], turn["end"])
        if right - left > overlap:
            overlap = right - left
            best = turn["speaker"]
    return best


def embed(extractor, samples, start, end):
    low = max(0, int(start * RATE))
    high = min(len(samples), int(end * RATE))
    if high <= low:
        return []
    clip = samples[low:high]
    stream = extractor.create_stream()
    stream.accept_waveform(sample_rate=RATE, waveform=clip)
    if hasattr(stream, "input_finished"):
        stream.input_finished()
    if hasattr(extractor, "is_ready") and not extractor.is_ready(stream):
        return []
    return [float(value) for value in extractor.compute(stream)]


def speakers_of(extractor, samples, turns):
    grouped = {}
    for turn in turns:
        grouped.setdefault(turn["speaker"], []).append(turn)
    speakers = []
    for speaker, own in grouped.items():
        taken = 0.0
        vectors = []
        for turn in own:
            if taken >= 30.0:
                break
            length = min(10.0, 30.0 - taken, turn["end"] - turn["start"])
            vector = embed(extractor, samples, turn["start"], turn["start"] + length)
            if vector:
                vectors.append(vector)
            taken += length
        if not vectors:
            speakers.append({"id": speaker, "embedding": []})
            continue
        mean = np.mean(np.asarray(vectors, dtype=np.float32), axis=0)
        norm = float(np.linalg.norm(mean))
        if norm > 1e-8:
            mean = mean / norm
        speakers.append({"id": speaker, "embedding": [float(value) for value in mean]})
    return speakers


def analyse(path):
    asr, diarizer, extractor = borrow()
    try:
        return _analyse(asr, diarizer, extractor, path)
    finally:
        release()


def _analyse(asr, diarizer, extractor, path):
    samples = read_audio(path)
    text, found, duration = transcribe_long(asr, samples)
    turns = diarize(diarizer, samples)
    for word in found:
        speaker = speaker_of(word, turns)
        if speaker is not None:
            word["speaker"] = speaker
    segments = []
    for turn in turns:
        pieces = [
            word["word"]
            for word in found
            if word.get("speaker") == turn["speaker"]
            and word["end"] > turn["start"]
            and word["start"] < turn["end"]
        ]
        segments.append(
            {
                "start": turn["start"],
                "end": turn["end"],
                "text": " ".join(pieces),
                "speaker": turn["speaker"],
            }
        )
    return {
        "task": "transcribe",
        "language": "ru",
        "duration": duration,
        "text": text,
        "words": found,
        "segments": segments,
        "speakers": speakers_of(extractor, samples, turns),
    }


def authorized(header):
    if header is None:
        return False
    token = header.strip()
    if token.lower().startswith("bearer "):
        token = token[7:].strip()
    return token == KEY


def read_file(handler):
    content_type = handler.headers.get("Content-Type", "")
    length = int(handler.headers.get("Content-Length", "0"))
    body = handler.rfile.read(length)
    marker = b""
    for part in content_type.split(";"):
        part = part.strip()
        if part.lower().startswith("boundary="):
            marker = part.split("=", 1)[1].strip('"').encode()
    if not marker:
        return None
    chunks = body.split(b"--" + marker)
    for chunk in chunks:
        if b"filename=" not in chunk:
            continue
        header, _, data = chunk.partition(b"\r\n\r\n")
        if not data:
            continue
        data = data[:-2] if data.endswith(b"\r\n") else data
        path = "/tmp/mediaa-speech.wav"
        with open(path, "wb") as handle:
            handle.write(data)
        return path
    return None


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.split("?", 1)[0] != "/health":
            self.send_error(404)
            return
        payload = json.dumps({"ok": True}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_POST(self):
        path = self.path.split("?", 1)[0]
        if path not in ("/v1/audio/transcriptions", "/v1/mediaa/transcribe"):
            self.send_error(404)
            return
        if not authorized(self.headers.get("Authorization")):
            self.send_error(401)
            return
        audio = read_file(self)
        if audio is None:
            self.send_error(400)
            return
        try:
            payload = json.dumps(analyse(audio), ensure_ascii=False).encode()
        except Exception as error:
            message = json.dumps({"error": str(error)}).encode()
            self.send_response(500)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(message)))
            self.end_headers()
            self.wfile.write(message)
            return
        finally:
            if os.path.exists(audio):
                os.remove(audio)
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, fmt, *args):
        print("%s - %s" % (self.address_string(), fmt % args), flush=True)


if __name__ == "__main__":
    ThreadingHTTPServer((HOST, PORT), Handler).serve_forever()
