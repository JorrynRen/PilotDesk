from __future__ import annotations

import random
from typing import Any

try:
    from .theme_models import RegistryTheme, ThemeCandidate
    from .theme_registry import load_cover_theme_registry
    from .theme_request_builder import normalize_theme_query, theme_query_to_dict
except ImportError:  # pragma: no cover - 兼容脚本直载
    from theme_models import RegistryTheme, ThemeCandidate
    from theme_registry import load_cover_theme_registry
    from theme_request_builder import normalize_theme_query, theme_query_to_dict


_RNG = random.SystemRandom()
_DEFAULT_CANDIDATE_COUNT = 3
_SINGLE_CANDIDATE_COUNT = 1
_CANDIDATE_RESPONSE_FIELDS = (
    "theme_id",
    "palette",
    "typography",
)
_CONTRACT_RESPONSE_FIELDS = (
    "theme_id",
    "theme_name",
    "style_sentence",
    "palette",
    "typography",
    "theme_query_snapshot",
)


def get_cover_theme_candidates(
    scene: str,
) -> dict[str, Any]:
    """scene 桶内随机抽 3 套（快速/兜底模式）。

    默认流程请用 `list_cover_theme_candidates` + `get_cover_theme_by_ids`，
    让模型基于 style_sentence 自主判断贴合度；仅在用户明确不要求视觉匹配
    时才调用本函数。
    """
    return _get_cover_theme_candidates_with_count(
        scene=scene,
        candidate_count=_DEFAULT_CANDIDATE_COUNT,
    )


def get_cover_theme_candidate(
    scene: str,
) -> dict[str, Any]:
    payload = _get_cover_theme_candidates_with_count(
        scene=scene,
        candidate_count=_SINGLE_CANDIDATE_COUNT,
    )
    return payload["candidates"][0]


def list_cover_theme_candidates(scene: str) -> dict[str, Any]:
    """列出指定 scene 下全部候选（theme_id / name / palette / typography）。

    模型据此直接挑出 3 套主题，无需二次拉取。
    """
    query = normalize_theme_query(
        scene=scene,
        candidate_count=_SINGLE_CANDIDATE_COUNT,
    )
    matched = [theme for theme in load_cover_theme_registry() if query.scene in theme.supported_scenes]
    return {
        "candidates": [
            _filter_payload(theme.candidate.to_dict(), _CANDIDATE_RESPONSE_FIELDS)
            for theme in matched
        ],
    }


def get_cover_theme_by_ids(theme_ids: list[str]) -> dict[str, Any]:
    """按 theme_id 批量拉取完整主题数据（含 palette / typography）。

    candidates 顺序与入参对齐；未命中的 id 放在 missing_ids 里回报。
    """
    if not theme_ids:
        return {"candidates": [], "missing_ids": []}

    candidates: list[dict[str, Any]] = []
    missing_ids: list[str] = []
    for theme_id in theme_ids:
        normalized_id = str(theme_id).strip()
        if not normalized_id:
            missing_ids.append(str(theme_id))
            continue
        candidate = _lookup_theme_candidate(normalized_id)
        if candidate is None:
            missing_ids.append(normalized_id)
            continue
        candidates.append(_serialize_candidate_response(candidate))
    return {"candidates": candidates, "missing_ids": missing_ids}


def build_theme_contract(
    candidate_payload: dict[str, Any],
    scene: str,
) -> dict[str, Any]:
    candidate = _resolve_candidate(candidate_payload)
    theme_query = normalize_theme_query(
        scene=scene,
        candidate_count=_SINGLE_CANDIDATE_COUNT,
    )
    contract_payload = {
        "theme_id": candidate.theme_id,
        "theme_name": candidate.name,
        "style_sentence": candidate.style_sentence,
        "palette": list(candidate.palette),
        "typography": candidate.typography.to_dict(),
        "theme_query_snapshot": theme_query_to_dict(theme_query),
    }
    if candidate.master_style is not None:
        contract_payload["master_style"] = candidate.master_style.to_dict()
    return _filter_payload(contract_payload, _CONTRACT_RESPONSE_FIELDS)


def _get_cover_theme_candidates_with_count(
    scene: str,
    candidate_count: int,
) -> dict[str, Any]:
    query = normalize_theme_query(
        scene=scene,
        candidate_count=candidate_count,
    )
    selected_themes = _select_random_themes(load_cover_theme_registry(), query.scene, query.candidate_count)
    return {"candidates": [_serialize_candidate_response(theme.candidate) for theme in selected_themes]}


def _serialize_candidate_response(candidate: ThemeCandidate) -> dict[str, Any]:
    return _filter_payload(candidate.to_dict(), _CANDIDATE_RESPONSE_FIELDS)


def _filter_payload(payload: dict[str, Any], allowed_fields: tuple[str, ...]) -> dict[str, Any]:
    return {field: payload[field] for field in allowed_fields if field in payload}


def _resolve_candidate(candidate_payload: dict[str, Any]) -> ThemeCandidate:
    if _is_complete_candidate_payload(candidate_payload):
        return ThemeCandidate.from_dict(candidate_payload)

    theme_id = str(candidate_payload.get("theme_id", "")).strip()
    if not theme_id:
        return ThemeCandidate.from_dict(candidate_payload)

    full_candidate = _lookup_theme_candidate(theme_id)
    if full_candidate is None:
        return ThemeCandidate.from_dict(candidate_payload)

    merged_payload = full_candidate.to_dict()
    merged_payload.update(candidate_payload)
    return ThemeCandidate.from_dict(merged_payload)


def _is_complete_candidate_payload(candidate_payload: dict[str, Any]) -> bool:
    return (
        "palette" in candidate_payload
        and "typography" in candidate_payload
        and "master_style" in candidate_payload
    )


def _lookup_theme_candidate(theme_id: str) -> ThemeCandidate | None:
    for registry_theme in load_cover_theme_registry():
        if registry_theme.candidate.theme_id == theme_id:
            return registry_theme.candidate
    return None


def _select_random_themes(
    registry: list[RegistryTheme],
    scene: str,
    candidate_count: int,
) -> list[RegistryTheme]:
    if not registry:
        return []

    matched = [theme for theme in registry if scene in theme.supported_scenes]
    fallback = [theme for theme in registry if scene not in theme.supported_scenes]

    _RNG.shuffle(matched)
    _RNG.shuffle(fallback)

    selected = matched[:candidate_count]
    if len(selected) < candidate_count:
        selected.extend(fallback[: candidate_count - len(selected)])
    return selected

