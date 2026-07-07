#!/usr/bin/env python3
"""Validate conpub's repo-root Codex and Claude plugin package."""

from __future__ import annotations

import json
import sys
import tomllib
from pathlib import Path


def fail(message: str) -> None:
    raise SystemExit(f"plugin package invalid: {message}")


def load_json(path: Path) -> dict:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        fail(f"missing {path}")
    except json.JSONDecodeError as error:
        fail(f"invalid JSON in {path}: {error}")


def require(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def main() -> None:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()
    cargo = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    version = cargo["package"]["version"]
    if len(sys.argv) > 2:
        require(version == sys.argv[2], f"Cargo version {version} does not match expected {sys.argv[2]}")

    codex_marketplace = load_json(root / ".agents/plugins/marketplace.json")
    claude_marketplace = load_json(root / ".claude-plugin/marketplace.json")
    codex_plugin = load_json(root / "plugins/conpub/.codex-plugin/plugin.json")
    claude_plugin = load_json(root / "plugins/conpub/.claude-plugin/plugin.json")
    skill = root / "plugins/conpub/skills/conpub/SKILL.md"
    openai_yaml = root / "plugins/conpub/skills/conpub/agents/openai.yaml"

    require(codex_marketplace.get("name") == "conpub", "Codex marketplace name must be conpub")
    codex_entries = codex_marketplace.get("plugins", [])
    require(len(codex_entries) == 1, "Codex marketplace must contain exactly one plugin")
    require(codex_entries[0].get("name") == "conpub", "Codex marketplace plugin name must be conpub")
    require(
        codex_entries[0].get("source")
        == {"source": "local", "path": "./plugins/conpub"},
        "Codex marketplace source must be ./plugins/conpub",
    )

    require(claude_marketplace.get("name") == "conpub", "Claude marketplace name must be conpub")
    claude_entries = claude_marketplace.get("plugins", [])
    require(len(claude_entries) == 1, "Claude marketplace must contain exactly one plugin")
    require(claude_entries[0].get("name") == "conpub", "Claude marketplace plugin name must be conpub")
    require(claude_entries[0].get("source") == "./plugins/conpub", "Claude marketplace source must be ./plugins/conpub")

    for label, manifest in (("Codex", codex_plugin), ("Claude", claude_plugin)):
        require(manifest.get("name") == "conpub", f"{label} plugin name must be conpub")
        require(manifest.get("version") == version, f"{label} plugin version must equal Cargo version {version}")
    require(claude_entries[0].get("version") == version, "Claude marketplace version must equal Cargo version")
    require(codex_plugin.get("skills") == "./skills/", "Codex plugin must expose ./skills/")

    require(skill.is_file(), f"missing canonical skill {skill}")
    require(openai_yaml.is_file(), f"missing skill metadata {openai_yaml}")
    require(not (root / ".agents/skills/conpub").exists(), "legacy .agents/skills/conpub copy must not exist")
    frontmatter = skill.read_text(encoding="utf-8").splitlines()[:4]
    require("name: conpub" in frontmatter, "canonical skill frontmatter name must be conpub")

    print(f"plugin package valid: conpub {version}")


if __name__ == "__main__":
    main()
