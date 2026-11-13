#!/usr/bin/env python3
"""Meshgrove's own two themes, made from SolidCraft's built-in palettes.

Meshgrove's look before the themes was egui's: neutral greys and a Breeze-like blue. SolidCraft's
built-in dark and light palettes (MIT OR Apache-2.0) are blue-greys with a stronger blue. This
takes every colour of theirs and
  - makes the blue-greys neutral, in the dark palette a little darker (panel #1b1b1b, as egui's),
  - turns the blues towards Breeze blue (#3daee9),
  - leaves the colours that mean something (errors, warnings, axes, analysis) as they are.

The palettes come from `cargo test -p meshgrove dump_builtin_palettes -- --ignored`
(scratch/builtin-dark.json, scratch/builtin-light.json). Run: python3 -I scripts/meshgrove-themes.py
"""
import colorsys
import json
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
TARGET = ROOT / "assets/themes-meshgrove"


def convert(color, dark):
    rgb, alpha = color[:7], color[7:]
    r, g, b = (int(rgb[i:i + 2], 16) / 255 for i in (1, 3, 5))
    hue, light, sat = colorsys.rgb_to_hls(r, g, b)
    degrees = hue * 360
    if sat < 0.36 and (195 <= degrees <= 245 or sat < 0.08):
        # A blue-grey surface, border or ink: neutral, and the dark surfaces a little darker.
        sat = 0.0
        if dark and light < 0.45:
            light = max(light - 0.07, 0.03)
    elif 204 <= degrees <= 228 and sat >= 0.36:
        # The interface's blue (accent, selection, focus): towards Breeze blue.
        hue = (degrees - 13) / 360
    r, g, b = colorsys.hls_to_rgb(hue, light, sat)
    return "#{:02x}{:02x}{:02x}".format(*(round(c * 255) for c in (r, g, b))) + alpha


def walk(value, dark):
    if isinstance(value, str) and value.startswith("#"):
        return convert(value, dark)
    if isinstance(value, list):
        return [walk(v, dark) for v in value]
    if isinstance(value, dict):
        return {k: walk(v, dark) for k, v in value.items()}
    return value


def main():
    TARGET.mkdir(exist_ok=True)
    for source, name, file, order in (("dark", "Meshgrove Dark", "meshgrove", 1), ("light", "Meshgrove Light", "meshgrove-light", 2)):
        base = json.loads((ROOT / f"scratch/builtin-{source}.json").read_text())
        dark = source == "dark"
        theme = {
            "format": base["format"],
            "name": name,
            "order": order,
            "provenance": "Meshgrove's own look: SolidCraft's built-in palette with neutral greys and Breeze blue",
            "base": base["base"],
            "colors": walk(base["colors"], dark),
            "radii": base["radii"],
            "borders": walk(base["borders"], dark),
        }
        (TARGET / f"{file}.json").write_text(json.dumps(theme, indent=2) + "\n")
        print(file, {k: theme["colors"][k] for k in ("panel", "app_bar", "text", "accent", "selected_bg", "border")})


main()
