#!/usr/bin/env python3
"""Generate YAS's compact runtime catalog from a genshin-db checkout."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
from pathlib import Path


ARTIFACT_SLOTS = (
    ("flower", "flower"),
    ("plume", "feather"),
    ("sands", "sand"),
    ("goblet", "goblet"),
    ("circlet", "head"),
)


def load_json(path: Path) -> dict:
    with path.open("r", encoding="utf-8") as stream:
        return json.load(stream)


def good_key(english_name: str) -> str:
    normalized = english_name.replace("'", "").replace("’", "")
    words = re.findall(r"[A-Za-z0-9]+", normalized)
    return "".join(word[:1].upper() + word[1:] for word in words)


def source_commit(repo: Path) -> str:
    result = subprocess.run(
        ["git", "-C", str(repo), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def source_game_version(repo: Path) -> str:
    package = load_json(repo / "package.json")
    match = re.search(r"Genshin Impact v([0-9.]+)", package.get("description", ""))
    return match.group(1) if match else "unknown"


def generate_artifact_sets(repo: Path) -> list[dict]:
    zh_dir = repo / "src" / "data" / "ChineseSimplified" / "artifacts"
    en_dir = repo / "src" / "data" / "English" / "artifacts"
    artifact_sets = []

    for zh_path in sorted(zh_dir.glob("*.json")):
        en_path = en_dir / zh_path.name
        if not en_path.exists():
            raise FileNotFoundError(f"Missing English artifact data for {zh_path.name}")

        zh = load_json(zh_path)
        en = load_json(en_path)
        pieces = []
        for source_key, slot in ARTIFACT_SLOTS:
            piece = zh.get(source_key)
            if piece:
                pieces.append({"name_zh_cn": piece["name"], "slot": slot})

        artifact_sets.append(
            {
                "id": zh["id"],
                "name_zh_cn": zh["name"],
                "name_en": en["name"],
                "good_key": good_key(en["name"]),
                "pieces": pieces,
            }
        )

    artifact_sets.sort(key=lambda item: item["id"])
    return artifact_sets


def generate_characters(repo: Path) -> list[dict]:
    zh_dir = repo / "src" / "data" / "ChineseSimplified" / "characters"
    en_dir = repo / "src" / "data" / "English" / "characters"
    characters = [
        {
            "id": 0,
            "name_zh_cn": "旅行者",
            "name_en": "Traveler",
            "good_key": "Traveler",
            "aliases_zh_cn": ["空", "荧"],
        }
    ]

    for zh_path in sorted(zh_dir.glob("*.json")):
        if zh_path.stem in {"aether", "lumine"}:
            continue

        en_path = en_dir / zh_path.name
        if not en_path.exists():
            raise FileNotFoundError(f"Missing English character data for {zh_path.name}")

        zh = load_json(zh_path)
        en = load_json(en_path)
        characters.append(
            {
                "id": zh["id"],
                "name_zh_cn": zh["name"],
                "name_en": en["name"],
                "good_key": good_key(en["name"]),
                "aliases_zh_cn": [],
            }
        )

    characters.sort(key=lambda item: item["id"])
    return characters


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("genshin_db", type=Path, help="Path to a genshin-db checkout")
    parser.add_argument("output", type=Path, help="Output catalog JSON path")
    args = parser.parse_args()

    repo = args.genshin_db.resolve()
    catalog = {
        "schema_version": 1,
        "source": {
            "repository": "https://github.com/theBowja/genshin-db",
            "commit": source_commit(repo),
            "game_version": source_game_version(repo),
        },
        "artifact_sets": generate_artifact_sets(repo),
        "characters": generate_characters(repo),
    }

    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("w", encoding="utf-8", newline="\n") as stream:
        json.dump(catalog, stream, ensure_ascii=False, indent=2)
        stream.write("\n")


if __name__ == "__main__":
    main()
