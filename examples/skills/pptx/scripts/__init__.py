"""PPTX scripts 包入口"""

from .theme_api_client import build_theme_contract, get_cover_theme_candidate, get_cover_theme_candidates

__all__ = [
    "list_cover_theme_candidates",
    "build_theme_contract",
    "get_cover_theme_candidate",
    "get_cover_theme_candidates",
]
