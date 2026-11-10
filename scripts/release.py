#!/usr/bin/env python3
"""Prepare and finish a release; the workflow alone receives the registry token."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import tomllib
import urllib.error
import urllib.request


ROOT = Path(__file__).resolve().parents[1]
VERSION = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\Z")


def run(*args, check=True):
    return subprocess.run(args, cwd=ROOT, text=True, check=check, capture_output=True)


def git(*args):
    return run("git", *args).stdout.strip()


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def package():
    return tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]


def version_bump(version, mode):
    require(VERSION.fullmatch(version), "Release versions must be plain major.minor.patch")
    require(mode in ("none", "patch", "minor", "major"), "Invalid version bump")
    if mode == "none":
        return version
    numbers = list(map(int, version.split(".")))
    index = {"major": 0, "minor": 1, "patch": 2}[mode]
    numbers[index] += 1
    numbers[index + 1:] = [0] * (2 - index)
    return ".".join(map(str, numbers))


def replace_manifest_version(text, old, new):
    """Edit only the package table, preserving the rest of the manifest."""
    pattern = re.compile(r"(?ms)(^\[package\]\s*\n)(.*?)(?=^\[|\Z)")
    matches = list(pattern.finditer(text))
    require(len(matches) == 1, "Expected one explicit [package] table")
    match = matches[0]
    body, count = re.subn(
        r'(?m)^(version\s*=\s*)"' + re.escape(old) + r'"(\s*(?:#.*)?)$',
        lambda m: m[1] + '"' + new + '"' + m[2], match[2],
    )
    require(count == 1, "Expected one explicit package version")
    return text[:match.start(2)] + body + text[match.end(2):]


def replace_lock_version(text, name, old, new):
    blocks = re.split(r"(?m)(?=^\[\[package\]\]\s*$)", text)
    changed = 0
    for index, block in enumerate(blocks):
        if not block.startswith("[[package]]"):
            continue
        item = tomllib.loads(block)["package"][0]
        if item.get("name") == name and "source" not in item:
            require(item["version"] == old, "Path dependency lockfile version disagrees with Cargo.toml")
            blocks[index], count = re.subn(
                r'(?m)^version = "' + re.escape(old) + r'"$',
                'version = "' + new + '"', block,
            )
            require(count == 1, "Expected one path package version in lockfile")
            changed += 1
    require(changed == 1, "Expected one zmime path package in lockfile")
    return "".join(blocks)


def output(**values):
    destination = os.environ.get("GITHUB_OUTPUT")
    lines = "".join(f"{key}={value}\n" for key, value in values.items())
    if destination:
        with open(destination, "a", encoding="utf-8") as stream:
            stream.write(lines)
    print(lines, end="")


def trusted_context():
    branch = os.environ["RELEASE_BRANCH"]
    require(os.environ.get("GITHUB_EVENT_NAME") == "workflow_dispatch", "Only manual default-branch releases are accepted")
    require(os.environ.get("GITHUB_REF") == "refs/heads/" + branch, "Release must originate on the default branch")
    run("git", "check-ref-format", "--branch", branch)
    require(not git("status", "--porcelain"), "The release checkout must be clean")
    git("fetch", "origin", f"+refs/heads/{branch}:refs/remotes/origin/{branch}", "--tags")
    return branch


def tag_target(tag):
    exists = run("git", "show-ref", "--verify", "--quiet", f"refs/tags/{tag}", check=False)
    if exists.returncode == 1:
        return None
    require(exists.returncode == 0, f"Could not inspect tag {tag}")
    result = run("git", "rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}", check=False)
    require(result.returncode == 0, f"Existing {tag} does not refer to a commit; refusing to reuse the tag")
    return result.stdout.strip()


def check_tag(tag, sha):
    target = tag_target(tag)
    require(target is None or target == sha, f"Existing {tag} points to a different commit; tags are never overwritten")
    return target


def prepare():
    branch = trusted_context()
    source = os.environ["GITHUB_SHA"]
    marker = f"Release-Run: {os.environ['GITHUB_REPOSITORY']}/{os.environ['GITHUB_RUN_ID']}"
    remote = git("rev-parse", f"origin/{branch}")
    message = git("log", "-1", "--format=%B", remote)
    resumed = marker in message.splitlines() and f"Release-Base: {source}" in message.splitlines()
    require(remote == source or resumed, "Default branch advanced after dispatch; start a fresh release run")
    if resumed:
        require(git("rev-parse", remote + "^") == source, "Release retry has an unexpected parent")
    git("checkout", "--detach", remote)
    old = package()["version"]
    version = version_bump(old, "none" if resumed else os.environ["RELEASE_BUMP"])
    if version != old:
        require(tag_target("v" + version) is None, "The requested version already has a tag")
        paths = ["Cargo.toml"]
        manifest = ROOT / "Cargo.toml"
        manifest.write_text(replace_manifest_version(manifest.read_text(), old, version))
        for filename in ("Cargo.lock", "comparison/Cargo.lock"):
            path = ROOT / filename
            if path.exists():
                path.write_text(replace_lock_version(path.read_text(), "zmime", old, version))
                paths.append(filename)
        git("config", "user.name", "github-actions[bot]")
        git("config", "user.email", "41898282+github-actions[bot]@users.noreply.github.com")
        git("add", "--", *paths)
        git("commit", "-m", f"chore: release v{version}\n\n{marker}\nRelease-Base: {source}")
        # A concurrent branch update rejects this push. Never force-push a release.
        git("push", "origin", f"HEAD:refs/heads/{branch}")
    sha = git("rev-parse", "HEAD")
    check_tag("v" + version, sha)
    output(sha=sha, version=version, tag="v" + version)


def fetch_json(url, *, token=None, method="GET", payload=None, missing_ok=False):
    headers = {"User-Agent": "zmime-release", "Accept": "application/json"}
    if token is not None:
        headers["Authorization"] = "Bearer " + token
        headers["X-GitHub-Api-Version"] = "2022-11-28"
    data = None if payload is None else json.dumps(payload).encode()
    if data is not None:
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        if error.code == 404 and missing_ok:
            return None
        raise RuntimeError(f"HTTP {error.code} from {url}; release stopped") from None


def registry_record(name, version):
    """The public sparse index provides the checksum of the immutable archive."""
    require(name == "zmime", "This publisher is restricted to zmime")
    url = f"https://index.crates.io/{name[:2]}/{name[2:4]}/{name}"
    request = urllib.request.Request(url, headers={"User-Agent": "zmime-release", "Cache-Control": "no-cache"})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            lines = response.read(8 * 1024 * 1024 + 1)
            require(len(lines) <= 8 * 1024 * 1024, "Registry metadata exceeds the safety bound")
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return None
        raise RuntimeError(f"Registry returned HTTP {error.code}; version existence is unknown") from None
    for line in lines.splitlines():
        record = json.loads(line)
        if record["vers"] == version:
            return record
    return None


def check_registry(archive, record):
    if record is None:
        return False
    require(not record.get("yanked", False), "This version is yanked; publish a new version")
    checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
    require(record.get("cksum") == checksum, "Published version has different archive bytes; refusing to reuse the version")
    return True


def release_state():
    branch = trusted_context()
    sha = os.environ["RELEASE_SHA"]
    version = os.environ["RELEASE_VERSION"]
    tag = os.environ["RELEASE_TAG"]
    require(re.fullmatch(r"[0-9a-f]{40}", sha), "Invalid release commit")
    require(VERSION.fullmatch(version) and tag == "v" + version, "Invalid release version/tag pair")
    require(git("rev-parse", "HEAD") == sha, "Checkout differs from the tested release commit")
    run("git", "merge-base", "--is-ancestor", sha, f"origin/{branch}")
    manifest = package()
    require(manifest["name"] == "zmime" and manifest["version"] == version, "Manifest differs from the tested release version")
    check_tag(tag, sha)
    archive = ROOT / "target" / "package" / f"zmime-{version}.crate"
    require(archive.is_file(), "The tested crate must be packaged before publishing")
    return sha, version, tag, archive


def registry_check():
    _, version, _, archive = release_state()
    published = check_registry(archive, registry_record("zmime", version))
    output(published=str(published).lower())


def finalize():
    sha, version, tag, archive = release_state()
    for attempt in range(7):
        if check_registry(archive, registry_record("zmime", version)):
            break
        if attempt == 6:
            raise RuntimeError("Publication is not visible in the registry yet; rerun with bump=none to finish")
        time.sleep(5)
    if tag_target(tag) is None:
        git("config", "user.name", "github-actions[bot]")
        git("config", "user.email", "41898282+github-actions[bot]@users.noreply.github.com")
        git("tag", "--annotate", tag, "--message", f"zmime {version}", sha)
        git("push", "origin", f"refs/tags/{tag}")
    repo = os.environ["GITHUB_REPOSITORY"]
    require(re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo), "Invalid repository identifier")
    token = os.environ["GH_TOKEN"]
    base = f"https://api.github.com/repos/{repo}/releases"
    existing = fetch_json(base + "/tags/" + tag, token=token, missing_ok=True)
    if existing is None:
        existing = fetch_json(base, token=token, method="POST", payload={
            "tag_name": tag, "target_commitish": sha, "name": f"zmime {version}",
            "generate_release_notes": True, "draft": False, "prerelease": False,
        })
    else:
        require(existing["tag_name"] == tag and not existing["prerelease"], "Existing GitHub Release has incompatible metadata")
        if existing["draft"]:
            existing = fetch_json(base + "/" + str(existing["id"]), token=token, method="PATCH", payload={"draft": False})
    print(existing["html_url"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("prepare", "registry-check", "finalize"))
    args = parser.parse_args()
    try:
        {"prepare": prepare, "registry-check": registry_check, "finalize": finalize}[args.command]()
    except (RuntimeError, KeyError, ValueError, OSError, subprocess.CalledProcessError) as error:
        # Do not print subprocess environments or request headers containing credentials.
        print(f"Release failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
