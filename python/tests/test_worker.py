from __future__ import annotations

from dataclasses import dataclass

import pytest
from herdr_stt_worker.worker import TranscriptionSettings, make_result, prepare_hints, transcribe


@dataclass
class Segment:
    text: str
    start: float = 0.0
    end: float = 1.0
    avg_logprob: float = -0.2
    no_speech_prob: float = 0.01
    words: list[Word] | None = None


@dataclass
class Word:
    word: str
    probability: float
    start: float = 0.0
    end: float = 0.5


@dataclass
class Info:
    language: str = "ru"
    language_probability: float = 0.93
    duration: float = 4.0
    duration_after_vad: float = 3.5


class FakeModel:
    def __init__(self) -> None:
        self.options = None

    def transcribe(self, _path: str, **options):
        self.options = options
        return iter([Segment(" Привет"), Segment(" world.")]), Info()


def test_make_result_consumes_segments() -> None:
    result = make_result(
        iter([Segment(" one", words=[Word(" one", 0.91)]), Segment(" two")]), Info()
    )
    assert result["text"] == "one two"
    assert result["language"] == "ru"
    assert result["duration_after_vad_seconds"] == 3.5
    assert result["segments"][0]["avg_logprob"] == -0.2
    assert result["segments"][0]["words"][0]["probability"] == 0.91


def test_transcribe_passes_multilingual_vad_and_hotwords() -> None:
    model = FakeModel()
    result = transcribe(
        model,
        {"audio_path": "/tmp/test.wav", "hotwords": ["Herdr", "Redis", "Herdr"]},
        TranscriptionSettings(
            initial_prompt="Technical dictation",
            condition_on_previous_text=False,
            word_timestamps=True,
        ),
    )
    assert result["text"] == "Привет world."
    assert model.options["language"] is None
    assert model.options["multilingual"] is True
    assert model.options["vad_filter"] is True
    assert model.options["hotwords"] == "Herdr, Redis"
    assert model.options["initial_prompt"] == "Technical dictation"
    assert model.options["condition_on_previous_text"] is False
    assert model.options["word_timestamps"] is True


def test_rejects_invalid_hotwords() -> None:
    with pytest.raises(ValueError, match="hotwords"):
        transcribe(
            FakeModel(),
            {"audio_path": "/tmp/test.wav", "hotwords": "Herdr"},
            TranscriptionSettings(),
        )


def test_request_context_is_per_transcription_and_language_disables_multilingual() -> None:
    model = FakeModel()
    settings = TranscriptionSettings(initial_prompt="Default background")
    transcribe(model, {
        "audio_path": "test.wav", "initial_prompt": "Current project", "language": "uk"
    }, settings)
    assert model.options["initial_prompt"] == "Current project"
    assert model.options["language"] == "uk"
    assert model.options["multilingual"] is False
    transcribe(model, {"audio_path": "test.wav"}, settings)
    assert model.options["initial_prompt"] == "Default background"
    assert model.options["language"] is None
    assert model.options["multilingual"] is True


@pytest.mark.parametrize("field,value", [("initial_prompt", 12), ("language", "invalid")])
def test_rejects_invalid_recognition_context(field, value) -> None:
    with pytest.raises((TypeError, ValueError), match=field):
        transcribe(FakeModel(), {"audio_path": "test.wav", field: value}, TranscriptionSettings())


def test_prompt_and_hotwords_share_budget_and_preserve_priority() -> None:
    from types import SimpleNamespace

    class Tokenizer:
        def encode(self, text, **_options):
            return SimpleNamespace(ids=[ord(char) for char in text])

    model = SimpleNamespace(hf_tokenizer=Tokenizer(), max_length=40)
    prompt, hotwords, truncated = prepare_hints(model, "Specific context first", "Redis, PostgreSQL")
    assert hotwords == "Redis"
    assert len(prompt) + len(" " + hotwords) <= 19
    assert "".join(map(chr, prompt)).startswith(" Specific")
    assert truncated
    prompt, hotwords, truncated = prepare_hints(model, "Short", "Rust")
    assert prompt == [ord(char) for char in " Short"]
    assert hotwords == "Rust"
    assert not truncated
