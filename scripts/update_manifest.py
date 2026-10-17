#!/usr/bin/env python3
"""Regenerate the ChannelFlow plugin repository's manifest.json.

The manifest mirrors the Jellyfin plugin-catalog shape: one entry per plugin,
each carrying a `versions[]` list and an `imageUrl` banner. Every released
version becomes one version entry whose `artifacts` are the per-platform zips
from that tag's GitHub release, each with its sha256 checksum and byte size.

Deterministic: a version's `timestamp` comes from the release's `publishedAt`,
never from the clock, so regenerating with nothing changed writes (and commits)
nothing. This runs locally and in CI on every plugin release.

Requires `gh` (authenticated), `curl`, and `sha256sum` on the PATH. Set
`RAW_BASE` (e.g. https://raw.githubusercontent.com/owner/repo/main) when not
running from the canonical git checkout.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from collections import OrderedDict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "manifest.json"


def version_key(value: str) -> tuple[int, ...]:
    """Order versions the way a person reads them.

    A plain string sort puts "2.0.4" after "2.0.33", and the base installs the
    *last* entry of `versions[]` — so a string-sorted manifest hands out an
    older plugin than the newest release. Compare the numeric parts instead.
    """
    parts: list[int] = []
    for part in value.strip().lstrip("v").split("."):
        digits = "".join(ch for ch in part if ch.isdigit())
        parts.append(int(digits) if digits else 0)
    return tuple(parts)


def run(*args: str) -> str:
    return subprocess.run(
        args, capture_output=True, text=True, check=True
    ).stdout.strip()


def gh(*args: str) -> str:
    return run("gh", *args)


def raw_base() -> str:
    """The raw.githubusercontent.com base for this repo, for imageUrl etc."""
    if os.environ.get("RAW_BASE"):
        return os.environ["RAW_BASE"].rstrip("/")
    url = run("git", "remote", "get-url", "origin")
    if url.startswith("git@github.com:"):
        url = "https://github.com/" + url[len("git@github.com:") :]
    url = url.removesuffix(".git")
    if "github.com/" in url:
        return f"https://raw.githubusercontent.com/{url.split('github.com/')[-1]}/main"
    return ""


def try_release(tag: str) -> dict | None:
    """The release's assets + publishedAt, or None when it does not exist."""
    try:
        raw = gh("release", "view", tag, "--json", "assets,publishedAt", "-q", ".")
        return json.loads(raw) if raw else None
    except subprocess.CalledProcessError:
        return None


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


def released_versions(slug: str) -> list[str]:
    """Every version of a plugin that has a release, from its tags.

    The catalog is rebuilt from the release list, not from plugin.json alone:
    a version is current for only one commit, so the release that carried it is
    the only record that it exists.
    """
    out = []
    tags = gh("release", "list", "--limit", "200", "--json", "tagName", "--jq", ".[].tagName")
    prefix = f"{slug}-v"
    for tag in tags.splitlines():
        tag = tag.strip()
        if tag.startswith(prefix):
            out.append(tag[len(prefix) :])
    return out


def plugin_json_at(tag: str, slug: str, fallback: dict) -> dict:
    """A version's own plugin.json, from its tag, so its base-version bounds are
    the ones it shipped with. Falls back to the working tree (tag not fetched,
    or the script running outside a checkout)."""
    try:
        return json.loads(run("git", "show", f"{tag}:plugins/{slug}/plugin.json"))
    except Exception:
        return fallback


def version_entry(slug: str, version: str, plugin: dict) -> dict | None:
    tag = f"{slug}-v{version}"
    release = try_release(tag)
    if not release:
        print(
            f"::warning::no release found for {tag}; keeping whatever "
            "manifest.json already had for that version",
            file=sys.stderr,
        )
        return None

    prefix = f"{plugin['id']}-v{version}-"
    artifacts: dict[str, dict] = {}
    for asset in release.get("assets", []):
        name = asset.get("name", "")
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
        "version": version,
        "min_base_version": plugin.get("min_base_version", ""),
        "max_base_version": plugin.get("max_base_version", ""),
        "timestamp": release.get("publishedAt") or "",
        "artifacts": dict(sorted(artifacts.items())),
    }


def build(loaded: list[dict]) -> list[dict]:
    base = raw_base()
    by_id = OrderedDict((entry["id"], entry) for entry in loaded)
    for plugin_json in sorted(ROOT.glob("plugins/*/plugin.json")):
        plugin = json.loads(plugin_json.read_text())
        slug = plugin_json.parent.name
        had_entry = plugin["id"] in by_id
        entry = by_id.setdefault(plugin["id"], new_entry(plugin))
        refresh_meta(entry, plugin)
        banner = ROOT / "banners" / f"{plugin['id']}.png"
        entry["imageUrl"] = f"{base}/banners/{plugin['id']}.png" if (base and banner.exists()) else entry.get("imageUrl", "")

        version = version_entry(slug, plugin["version"], plugin)
        if version is not None:
            entry["versions"] = [
                v for v in entry["versions"] if v["version"] != version["version"]
            ]
            entry["versions"].append(version)

        # Backfill: every other tag that has a release belongs in the catalog
        # too. A version is current for only one commit, so without this the
        # manifest loses any version whose release landed in a manifest job
        # that raced the asset upload — and the base can no longer install it.
        known = {v["version"] for v in entry["versions"]}
        for other in sorted(set(released_versions(slug)) - known, key=version_key):
            built = version_entry(
                slug, other, plugin_json_at(f"{slug}-v{other}", slug, plugin)
            )
            if built is not None:
                entry["versions"].append(built)

        if not entry["versions"]:
            # A plugin with no release yet stays out of the catalog entirely;
            # it appears here on its first release (its tag + assets land).
            del by_id[plugin["id"]]
            continue
        entry["versions"].sort(key=lambda v: version_key(v["version"]))
    return list(by_id.values())


def new_entry(plugin: dict) -> dict:
    return {
        "id": plugin["id"],
        "guid": plugin["id"],
        "name": plugin.get("name", plugin["id"]),
        "description": plugin.get("description", ""),
        "owner": plugin.get("author", ""),
        "category": plugin.get("category", ""),
        "homepage": plugin.get("homepage", ""),
        "imageUrl": "",
        "versions": [],
    }


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