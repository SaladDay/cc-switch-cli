#!/usr/bin/env python3

import json
import sys
from pathlib import Path


def asset_entry(release_dir: Path, base_url: str, filename: str):
    return {
        "url": f"{base_url}/{filename}",
        "signature": (release_dir / f"{filename}.minisig")
        .read_text(encoding="utf-8")
        .strip(),
    }


def file_exists(release_dir: Path, filename: str) -> bool:
    return (release_dir / filename).is_file() and (
        release_dir / f"{filename}.minisig"
    ).is_file()


def add_mac_platforms(manifest: dict, release_dir: Path, base_url: str):
    universal = "cc-switch-cli-darwin-universal.tar.gz"
    x64 = "cc-switch-cli-darwin-x64.tar.gz"
    arm64 = "cc-switch-cli-darwin-arm64.tar.gz"

    if file_exists(release_dir, x64):
        manifest["platforms"]["darwin-x86_64"] = asset_entry(release_dir, base_url, x64)
    elif file_exists(release_dir, universal):
        manifest["platforms"]["darwin-x86_64"] = asset_entry(
            release_dir, base_url, universal
        )

    if file_exists(release_dir, arm64):
        manifest["platforms"]["darwin-aarch64"] = asset_entry(
            release_dir, base_url, arm64
        )
    elif file_exists(release_dir, universal):
        manifest["platforms"]["darwin-aarch64"] = asset_entry(
            release_dir, base_url, universal
        )


def add_linux_platform(
    manifest: dict,
    release_dir: Path,
    base_url: str,
    platform_key: str,
    musl_name: str,
    glibc_name: str,
):
    if file_exists(release_dir, musl_name):
        entry: dict[str, object] = dict(asset_entry(release_dir, base_url, musl_name))
        if file_exists(release_dir, glibc_name):
            entry["variants"] = {
                "glibc": asset_entry(release_dir, base_url, glibc_name),
            }
        manifest["platforms"][platform_key] = entry
        return

    if file_exists(release_dir, glibc_name):
        manifest["platforms"][platform_key] = asset_entry(
            release_dir, base_url, glibc_name
        )


def main() -> int:
    if len(sys.argv) != 6:
        print(
            "Usage: generate_latest_json.py <release_dir> <version> <pub_date> <base_url> <notes>",
            file=sys.stderr,
        )
        return 1

    release_dir = Path(sys.argv[1]).resolve()
    version = sys.argv[2]
    pub_date = sys.argv[3]
    base_url = sys.argv[4].rstrip("/")
    notes = sys.argv[5]

    manifest = {
        "version": version,
        "notes": notes,
        "pub_date": pub_date,
        "platforms": {},
    }

    add_mac_platforms(manifest, release_dir, base_url)
    add_linux_platform(
        manifest,
        release_dir,
        base_url,
        "linux-x86_64",
        "cc-switch-cli-linux-x64-musl.tar.gz",
        "cc-switch-cli-linux-x64.tar.gz",
    )
    add_linux_platform(
        manifest,
        release_dir,
        base_url,
        "linux-aarch64",
        "cc-switch-cli-linux-arm64-musl.tar.gz",
        "cc-switch-cli-linux-arm64.tar.gz",
    )

    windows = "cc-switch-cli-windows-x64.zip"
    if file_exists(release_dir, windows):
        manifest["platforms"]["windows-x86_64"] = asset_entry(
            release_dir, base_url, windows
        )

    if not manifest["platforms"]:
        print("No signed release assets found to build latest.json", file=sys.stderr)
        return 1

    output_path = release_dir / "latest.json"
    output_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
