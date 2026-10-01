from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


@dataclass(slots=True)
class ThemeQuery:
    scene: str
    candidate_count: int = 3


@dataclass(slots=True)
class BackgroundStop:
    color: str
    position: int

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> BackgroundStop:
        return cls(color=str(payload["color"]), position=int(payload["position"]))

    def to_dict(self) -> dict[str, Any]:
        return {"color": self.color, "position": self.position}


@dataclass(slots=True)
class BackgroundLayer:
    type: str
    angle: int | None = None
    stops: list[BackgroundStop] = field(default_factory=list)
    color: str | None = None
    opacity: float | None = None
    x: str | None = None
    y: str | None = None
    size: str | None = None
    blend_mode: str | None = None
    width: str | None = None
    height: str | None = None
    offset_x: str | None = None
    offset_y: str | None = None
    spacing: str | None = None

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> BackgroundLayer:
        return cls(
            type=str(payload["type"]),
            angle=int(payload["angle"]) if payload.get("angle") is not None else None,
            stops=[BackgroundStop.from_dict(item) for item in payload.get("stops", [])],
            color=payload.get("color"),
            opacity=float(payload["opacity"]) if payload.get("opacity") is not None else None,
            x=payload.get("x"),
            y=payload.get("y"),
            size=payload.get("size"),
            blend_mode=payload.get("blend_mode"),
            width=payload.get("width"),
            height=payload.get("height"),
            offset_x=payload.get("offset_x"),
            offset_y=payload.get("offset_y"),
            spacing=payload.get("spacing"),
        )

    def to_dict(self) -> dict[str, Any]:
        payload: dict[str, Any] = {"type": self.type}
        if self.angle is not None:
            payload["angle"] = self.angle
        if self.stops:
            payload["stops"] = [item.to_dict() for item in self.stops]
        if self.color is not None:
            payload["color"] = self.color
        if self.opacity is not None:
            payload["opacity"] = self.opacity
        if self.x is not None:
            payload["x"] = self.x
        if self.y is not None:
            payload["y"] = self.y
        if self.size is not None:
            payload["size"] = self.size
        if self.blend_mode is not None:
            payload["blend_mode"] = self.blend_mode
        if self.width is not None:
            payload["width"] = self.width
        if self.height is not None:
            payload["height"] = self.height
        if self.offset_x is not None:
            payload["offset_x"] = self.offset_x
        if self.offset_y is not None:
            payload["offset_y"] = self.offset_y
        if self.spacing is not None:
            payload["spacing"] = self.spacing
        return payload


@dataclass(slots=True)
class CanvasStyle:
    background: str

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> CanvasStyle:
        return cls(background=str(payload["background"]))

    def to_dict(self) -> dict[str, Any]:
        return {"background": self.background}


@dataclass(slots=True)
class ColorTokens:
    title: str
    body: str
    accent: str
    secondary: str | None = None
    divider: str | None = None

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> ColorTokens:
        return cls(
            title=str(payload["title"]),
            body=str(payload["body"]),
            accent=str(payload["accent"]),
            secondary=payload.get("secondary"),
            divider=payload.get("divider"),
        )

    def to_dict(self) -> dict[str, Any]:
        payload: dict[str, Any] = {
            "title": self.title,
            "body": self.body,
            "accent": self.accent,
        }
        if self.secondary is not None:
            payload["secondary"] = self.secondary
        if self.divider is not None:
            payload["divider"] = self.divider
        return payload


@dataclass(slots=True)
class CompositionHints:
    layout_motif: str | None = None
    accent_mode: str | None = None
    surface_density: str | None = None
    composition_tension: str | None = None

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> CompositionHints:
        return cls(
            layout_motif=str(payload["layout_motif"]) if payload.get("layout_motif") is not None else None,
            accent_mode=str(payload["accent_mode"]) if payload.get("accent_mode") is not None else None,
            surface_density=str(payload["surface_density"]) if payload.get("surface_density") is not None else None,
            composition_tension=str(payload["composition_tension"]) if payload.get("composition_tension") is not None else None,
        )

    def to_dict(self) -> dict[str, Any]:
        payload: dict[str, Any] = {}
        if self.layout_motif is not None:
            payload["layout_motif"] = self.layout_motif
        if self.accent_mode is not None:
            payload["accent_mode"] = self.accent_mode
        if self.surface_density is not None:
            payload["surface_density"] = self.surface_density
        if self.composition_tension is not None:
            payload["composition_tension"] = self.composition_tension
        return payload


@dataclass(slots=True)
class MasterStyle:
    canvas: CanvasStyle
    background_layers: list[BackgroundLayer]
    color_tokens: ColorTokens
    composition: CompositionHints | None = None

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> MasterStyle:
        return cls(
            canvas=CanvasStyle.from_dict(payload["canvas"]),
            background_layers=[BackgroundLayer.from_dict(item) for item in payload.get("background_layers", [])],
            color_tokens=ColorTokens.from_dict(payload["color_tokens"]),
            composition=CompositionHints.from_dict(payload["composition"]) if payload.get("composition") is not None else None,
        )

    def to_dict(self) -> dict[str, Any]:
        payload: dict[str, Any] = {
            "canvas": self.canvas.to_dict(),
            "background_layers": [item.to_dict() for item in self.background_layers],
            "color_tokens": self.color_tokens.to_dict(),
        }
        if self.composition is not None:
            composition_payload = self.composition.to_dict()
            if composition_payload:
                payload["composition"] = composition_payload
        return payload


@dataclass(slots=True)
class Typography:
    display_font: str
    body_font: str

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> Typography:
        return cls(
            display_font=str(payload["display_font"]),
            body_font=str(payload["body_font"]),
        )

    def to_dict(self) -> dict[str, Any]:
        return {
            "display_font": self.display_font,
            "body_font": self.body_font,
        }


@dataclass(slots=True)
class ThemeCandidate:
    theme_id: str
    name: str
    palette: list[str]
    typography: Typography
    style_sentence: str = ""
    master_style: MasterStyle | None = None

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> ThemeCandidate:
        return cls(
            theme_id=str(payload["theme_id"]),
            name=str(payload["name"]),
            style_sentence=str(payload.get("style_sentence", "")),
            palette=[str(item) for item in payload["palette"]],
            master_style=MasterStyle.from_dict(payload["master_style"]) if payload.get("master_style") is not None else None,
            typography=Typography.from_dict(payload["typography"]),
        )

    def to_dict(self) -> dict[str, Any]:
        payload: dict[str, Any] = {
            "theme_id": self.theme_id,
            "name": self.name,
            "palette": list(self.palette),
            "typography": self.typography.to_dict(),
        }
        if self.style_sentence:
            payload["style_sentence"] = self.style_sentence
        if self.master_style is not None:
            payload["master_style"] = self.master_style.to_dict()
        return payload


@dataclass(slots=True)
class RegistryTheme:
    candidate: ThemeCandidate
    supported_scenes: list[str]

