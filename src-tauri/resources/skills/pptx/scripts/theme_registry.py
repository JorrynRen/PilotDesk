from __future__ import annotations

import json
from functools import lru_cache
from pathlib import Path

try:
    from .theme_models import RegistryTheme, ThemeCandidate
except ImportError:  # pragma: no cover - 兼容脚本直载
    from theme_models import RegistryTheme, ThemeCandidate


_REGISTRY_PATH = Path(__file__).resolve().parent.parent / "themes" / "cover_theme_registry.json"


@lru_cache(maxsize=1)
def load_cover_theme_registry() -> list[RegistryTheme]:
    with _REGISTRY_PATH.open("r", encoding="utf-8") as handle:
        payload = json.load(handle)
    return [_load_registry_theme(item) for item in payload["themes"]]


def registry_path() -> Path:
    return _REGISTRY_PATH


def _load_registry_theme(entry: dict[str, object]) -> RegistryTheme:
    supported_scenes = [str(item) for item in entry.get("supported_scenes", [])]
    candidate_payload = {k: v for k, v in entry.items() if k != "supported_scenes"}
    return RegistryTheme(
        candidate=ThemeCandidate.from_dict(candidate_payload),
        supported_scenes=supported_scenes,
    )

