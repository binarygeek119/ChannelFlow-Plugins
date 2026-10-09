#!/usr/bin/env python3
"""Regenerate the ChannelFlow plugin repository's manifest.json.

The manifest mirrors the Jellyfin plugin-catalog shape: one entry per plugin,
each carrying a `versions[]` list. Every released version becomes one version
entry whose `artifacts` are the per-platform zips from that tag's GitHub
release, each with its sha256 checksum and byte size. This runs locally and in
CI on every plugin release, so manifest.json never drifts from what is
actually published.

Requires `gh` (authenticated), `curl`, and `sha256sum` on the PATH.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from collections import OrderedDict
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "manifest.json"


def run(*args: str) -> str:
    return subprocess.run(
        args, capture_output=True, text=True, check=True
    ).stdout.strip()


def gh(*args: str) -> str:
    return run("gh", *args)


def sha256_of(url: str) -> str:
    """Download the url once and answer its sha256 in lowercase hex."""
    with tempfile.NamedTemporaryFile(delete=False) as tmp:
        path = tmp.name
    try:
        run("curl", "-fsSL", "-o", path, url)
        digest = run("sha256sum", path).split()[0]
    finally:
        os.unlink(path)
    return digest


def release_assets(tag: str) -> list[dict]:
    """Every asset on the release, or [] when the release does not exist."""
    raw = gh("release", "view", tag, "--json", "assets", "-q", ".assets")
    assets = json.loads(raw) if raw else []
    return [a for a in assets if a.get("name", "").endswith(".zip")]


def version_entry(slug: str, plugin: dict) -> dict | None:
    tag = f"{slug}-v{plugin['version']}"
    assets = release_assets(tag)
    if not assets:
        print(
            f"::warning::no release assets for {tag}; "
            "keeping whatever manifest.json already had for it",
            file=sys.stderr,
        )
        return None

    prefix = f"{plugin['id']}-v{plugin['version']}-"
    artifacts: dict[str, dict] = {}
    for asset in assets:
        name = asset["name"]
        if not name.startswith(prefix) or not name.endswith(".zip"):
            continue
        rid = name[len(prefix) : -len(".zip")]
        artifacts[rid] = {
            "url": asset["url"],
            "checksum": f"sha256:{sha256_of(asset['url'])}",
            "size": asset["size"],
        }
    if not artifacts:
        print(f"::warning::no zips matched {prefix}*.zip on {tag}", file=sys.stderr)
        return None

    return {
        "version": plugin["version"],
        "min_base_version": plugin.get("min_base_version", ""),
        "max_base_version": plugin.get("max_base_version", ""),
        "timestamp": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "artifacts": dict(sorted(artifacts.items())),
    }


def build(loaded: list[dict]) -> list[dict]:
    by_id = OrderedDict((entry["id"], entry) for entry in loaded)
    for plugin_json in sorted(ROOT.glob("plugins/*/plugin.json")):
        plugin = json.loads(plugin_json.read_text())
        slug = plugin_json.parent.name
        entry = by_id.setdefault(
            plugin["id"],
            {
                "id": plugin["id"],
                "guid": plugin["id"],
                "name": plugin.get("name", plugin["id"]),
                "description": plugin.get("description", ""),
                "owner": plugin.get("author", ""),
                "category": plugin.get("category", ""),
                "homepage": plugin.get("homepage", ""),
                "versions": [],
            },
        )
        refresh_meta(entry, plugin)
        version = version_entry(slug, plugin)
        if version is None:
            continue
        entry["versions"] = [v for v in entry["versions"] if v["version"] != version["version"]]
        entry["versions"].append(version)
        entry["versions"].sort(key=lambda v: v["version"])
    return list(by_id.values())


def refresh_meta(entry: dict, plugin: dict) -> None:
    for field, src in (
        ("name", "name"),
        ("description", "description"),
        ("owner", "author"),
        ("category", "category"),
        ("homepage", "homepage"),
    ):
        if plugin.get(src):
            entry[field] = plugin[src]


def main() -> int:
    loaded: list[dict] = []
    if MANIFEST.exists():
        loaded = json.loads(MANIFEST.read_text())
    entries = build(loaded)
    payload = json.dumps(entries, indent=2) + "\n"
    if MANIFEST.exists() and MANIFEST.read_text() == payload:
        print("manifest.json is unchanged")
        return 0
    MANIFEST.write_text(payload)
    print("manifest.json updated")
    return 0


if __name__ == "__main__":
    sys.exit(main())