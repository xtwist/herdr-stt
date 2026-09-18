from __future__ import annotations

import argparse
import json
import logging
import sys
from collections.abc import Iterable
from dataclasses import dataclass
from typing import Any

LOG = logging.getLogger("herdr_stt_worker")


@dataclass(frozen=True)
class TranscriptionSettings:
    beam_size: int = 3
    multilingual: bool = True
    vad_filter: bool = True
    min_silence_duration_ms: int = 500
    initial_prompt: str = ""
    condition_on_previous_text: bool = True
    word_timestamps: bool = False


def disable_onnx_telemetry() -> None:
    try:
        import onnxruntime

        onnxruntime.disable_telemetry_events()
    except (ImportError, AttributeError):
        LOG.debug("ONNX Runtime telemetry API is unavailable")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Long-lived faster-whisper JSON-lines worker")
    parser.add_argument("--model", default="small")
    parser.add_argument("--device", default="cuda")
    parser.add_argument("--compute-type", default="int8_float16")
    parser.add_argument("--cpu-compute-type", default="int8")
    parser.add_argument("--cpu-fallback", action="store_true")
    parser.add_argument("--beam-size", type=int, default=3)
    parser.add_argument("--multilingual", action="store_true")
    parser.add_argument("--no-vad-filter", action="store_true")
    parser.add_argument("--min-silence-duration-ms", type=int, default=500)
    parser.add_argument("--initial-prompt", default="")
    parser.add_argument("--no-condition-on-previous-text", action="store_true")
    parser.add_argument("--word-timestamps", action="store_true")
    parser.add_argument("--download-only", action="store_true")
    return parser.parse_args()


def emit(message: dict[str, Any]) -> None:
    sys.stdout.write(json.dumps(message, ensure_ascii=False, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def load_model(args: argparse.Namespace) -> tuple[Any, str, str]:
    from faster_whisper import WhisperModel

    try:
        model = WhisperModel(args.model, device=args.device, compute_type=args.compute_type)
        return model, args.device, args.compute_type
    except Exception as error:
        if not args.cpu_fallback or args.device == "cpu":
            raise
        LOG.warning("CUDA model load failed; falling back to CPU int8: %s", error)
        model = WhisperModel(args.model, device="cpu", compute_type=args.cpu_compute_type)
        return model, "cpu", args.cpu_compute_type


def transcribe(
    model: Any, request: dict[str, Any], settings: TranscriptionSettings
) -> dict[str, Any]:
    audio_path = request.get("audio_path")
    if not isinstance(audio_path, str) or not audio_path:
        raise ValueError("audio_path must be a non-empty string")
    raw_hotwords = request.get("hotwords", [])
    if not isinstance(raw_hotwords, list) or not all(
        isinstance(word, str) for word in raw_hotwords
    ):
        raise ValueError("hotwords must be a list of strings")
    hotwords = ", ".join(dict.fromkeys(word.strip() for word in raw_hotwords if word.strip()))
    initial_prompt = request.get("initial_prompt", settings.initial_prompt)
    if not isinstance(initial_prompt, str):
        raise TypeError("initial_prompt must be a string")
    prompt, hotwords, truncated = prepare_hints(model, initial_prompt, hotwords)
    language = request.get("language")
    if language is not None:
        from faster_whisper.tokenizer import _LANGUAGE_CODES

        if not isinstance(language, str) or language not in _LANGUAGE_CODES:
            raise ValueError("language must be a supported Whisper language code or null")
    segments, info = model.transcribe(
        audio_path,
        task="transcribe",
        language=language,
        beam_size=settings.beam_size,
        multilingual=settings.multilingual and language is None,
        vad_filter=settings.vad_filter,
        vad_parameters={"min_silence_duration_ms": settings.min_silence_duration_ms},
        hotwords=hotwords or None,
        initial_prompt=prompt or None,
        condition_on_previous_text=settings.condition_on_previous_text,
        word_timestamps=settings.word_timestamps,
    )
    result = make_result(segments, info)
    result["context_truncated"] = truncated
    return result


def prepare_hints(model: Any, prompt: str, hotwords: str) -> tuple[Any, str, bool]:
    """Share Whisper's previous-text budget between context and hotwords.

    faster-whisper 1.2.1 otherwise caps each independently, which can consume the
    entire decoder window. Keep the start: hints are ordered by specificity.
    """
    tokenizer = getattr(model, "hf_tokenizer", None)
    if tokenizer is None:
        return prompt, hotwords, False
    budget = model.max_length // 2 - 1
    prompt_tokens = tokenizer.encode(" " + prompt.strip(), add_special_tokens=False).ids if prompt else []
    terms = [term.strip() for term in hotwords.split(",") if term.strip()]
    word_budget = budget // 2 if prompt_tokens else budget
    selected = []
    word_tokens = []
    for term in terms:
        candidate = tokenizer.encode(" " + ", ".join([*selected, term]), add_special_tokens=False).ids
        if len(candidate) > word_budget:
            break
        selected.append(term)
        word_tokens = candidate
    prompt_budget = budget - len(word_tokens)
    truncated = len(selected) < len(terms) or len(prompt_tokens) > prompt_budget
    return prompt_tokens[:prompt_budget], ", ".join(selected), truncated


def make_result(segments: Iterable[Any], info: Any) -> dict[str, Any]:
    materialized = list(segments)
    text = "".join(segment.text for segment in materialized).strip()
    segment_results = []
    for segment in materialized:
        words = getattr(segment, "words", None) or []
        segment_results.append(
            {
                "start": float(getattr(segment, "start", 0.0)),
                "end": float(getattr(segment, "end", 0.0)),
                "text": str(getattr(segment, "text", "")),
                "avg_logprob": float(getattr(segment, "avg_logprob", 0.0)),
                "no_speech_prob": float(getattr(segment, "no_speech_prob", 0.0)),
                "words": [
                    {
                        "start": float(getattr(word, "start", 0.0)),
                        "end": float(getattr(word, "end", 0.0)),
                        "word": str(getattr(word, "word", "")),
                        "probability": float(getattr(word, "probability", 0.0)),
                    }
                    for word in words
                ],
            }
        )
    return {
        "text": text,
        "language": getattr(info, "language", None),
        "language_probability": getattr(info, "language_probability", None),
        "duration_seconds": float(getattr(info, "duration", 0.0)),
        "duration_after_vad_seconds": _optional_float(getattr(info, "duration_after_vad", None)),
        "segments": segment_results,
    }


def _optional_float(value: Any) -> float | None:
    return None if value is None else float(value)


def serve(model: Any, settings: TranscriptionSettings) -> None:
    for line in sys.stdin:
        request: Any = None
        try:
            request = json.loads(line)
            request_id = request.get("id")
            if request.get("method") != "transcribe":
                raise ValueError(f"unknown method: {request.get('method')}")
            result = transcribe(model, request, settings)
            emit({"type": "response", "id": request_id, "ok": True, "result": result})
        except Exception as error:
            LOG.exception("transcription request failed")
            emit(
                {
                    "type": "response",
                    "id": request.get("id") if isinstance(request, dict) else None,
                    "ok": False,
                    "error": str(error),
                }
            )


def main() -> int:
    logging.basicConfig(level=logging.INFO, stream=sys.stderr)
    disable_onnx_telemetry()
    args = parse_args()
    try:
        model, device, compute_type = load_model(args)
        if args.download_only:
            LOG.info("model %s is available on %s/%s", args.model, device, compute_type)
            return 0
        emit(
            {
                "type": "ready",
                "model": args.model,
                "device": device,
                "compute_type": compute_type,
            }
        )
        serve(
            model,
            TranscriptionSettings(
                beam_size=args.beam_size,
                multilingual=args.multilingual,
                vad_filter=not args.no_vad_filter,
                min_silence_duration_ms=args.min_silence_duration_ms,
                initial_prompt=args.initial_prompt,
                condition_on_previous_text=not args.no_condition_on_previous_text,
                word_timestamps=args.word_timestamps,
            ),
        )
        return 0
    except Exception as error:
        LOG.exception("worker startup failed")
        emit({"type": "fatal", "error": str(error)})
        return 1
