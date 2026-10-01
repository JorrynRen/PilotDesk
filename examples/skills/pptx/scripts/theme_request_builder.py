from __future__ import annotations

try:
    from .theme_models import ThemeQuery
except ImportError:  # pragma: no cover - 兼容脚本直载
    from theme_models import ThemeQuery


SUPPORTED_SCENES = frozenset(
    {
        "Business_Corporate",
        "Technology_Innovation",
        "Health_BioTech",
        "Public_Government",
        "Industry_Engineering",
        "Empathy_NonProfit",
        "Culture_History",
        "Education_Academia",
        "Fashion_Lifestyle",
        "Nature_Sustainability",
    }
)

_MAX_CANDIDATE_COUNT = 3


def normalize_theme_query(
    scene: str,
    candidate_count: int = 3,
) -> ThemeQuery:
    normalized_scene = _normalize_scene(scene)
    normalized_count = _normalize_candidate_count(candidate_count)

    return ThemeQuery(
        scene=normalized_scene,
        candidate_count=normalized_count,
    )


def theme_query_to_dict(query: ThemeQuery) -> dict[str, object]:
    return {
        "scene": query.scene,
        "candidate_count": query.candidate_count,
    }


def _canonical_scene(raw: str) -> str | None:
    if raw in SUPPORTED_SCENES:
        return raw
    for s in SUPPORTED_SCENES:
        if s.casefold() == raw.casefold():
            return s
    return None


def _normalize_scene(scene: str) -> str:
    raw = (scene or "").strip()
    if not raw:
        raise ValueError("scene must be a non-empty string")

    canon = _canonical_scene(raw)
    if canon is not None:
        return canon

    supported_values = ", ".join(sorted(SUPPORTED_SCENES))
    raise ValueError(f"invalid scene: {scene!r}. supported values: {supported_values}")

def _normalize_candidate_count(candidate_count: int) -> int:
    try:
        normalized = int(candidate_count)
    except (TypeError, ValueError) as exc:
        raise ValueError("candidate_count must be an integer") from exc

    if normalized < 1 or normalized > _MAX_CANDIDATE_COUNT:
        raise ValueError(f"candidate_count must be between 1 and {_MAX_CANDIDATE_COUNT}")
    return normalized
