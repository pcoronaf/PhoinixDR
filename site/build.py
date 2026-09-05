#!/usr/bin/env python3
"""Builds the PhoinixDR site: copies the static pages under site/ and
renders the Markdown documentation (docs/, README, CHANGELOG,
CONTRIBUTING, CODE_OF_CONDUCT, SECURITY) to HTML with the same layout.

Usage: python3 site/build.py --out _site
Requires the `markdown` package (pip install markdown).
"""

from __future__ import annotations

import argparse
import html
import os
import re
import shutil
from pathlib import Path

import markdown

ROOT = Path(__file__).resolve().parent.parent
SITE = ROOT / "site"
REPO_URL = "https://github.com/pcoronaf/PhoinixDR"

# Markdown sources rendered into the site, keyed by their output path.
DOC_ROOTS = {
    "docs": ROOT / "docs",
}
TOP_LEVEL = {
    "README.md": "docs/readme.html",
    "CHANGELOG.md": "docs/changelog.html",
    "CONTRIBUTING.md": "docs/contributing.html",
    "CODE_OF_CONDUCT.md": "docs/code-of-conduct.html",
    "SECURITY.md": "docs/security.html",
}


def layout(title: str, body: str, depth: int) -> str:
    rel = "../" * depth
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{html.escape(title)} · PhoinixDR</title>
<link rel="icon" href="{rel}assets/favicon.png">
<link rel="stylesheet" href="{rel}styles.css">
</head>
<body>
<header class="top">
  <a class="brand" href="{rel}index.html"><img src="{rel}assets/logo-mark.png" alt="" width="28" height="28"> PhoinixDR</a>
  <nav>
    <a href="{rel}docs/index.html">Documentation</a>
    <a href="{rel}docs/user-guide/desktop.html">User guide</a>
    <a href="{rel}download.html">Download</a>
    <a href="{rel}community.html">Community</a>
    <a href="{REPO_URL}">GitHub</a>
  </nav>
</header>
<main class="doc">
{body}
</main>
<footer class="foot">
  <p class="disclaimer">PhoinixDR is provided “as is” and is used entirely at your own risk. Data recovery is inherently uncertain, and improper use may result in permanent data loss or damage. Always work from a copy or disk image when possible and recover files to a different storage device. <a href="{rel}docs/disclaimer.html">Read the disclaimer</a>.</p>
  <p>PhoinixDR · Open Source Data Recovery · by <a href="https://github.com/pcoronaf">@pcoronaf</a> · MIT OR Apache-2.0 ·
  <a href="{rel}docs/about/vibecoded.html">Yes, PHOINIX is vibecoded</a> ·
  <a href="{rel}docs/about/origin.html">Where PHOINIX came from</a></p>
</footer>
</body>
</html>
"""


def first_heading(text: str, fallback: str) -> str:
    m = re.search(r"^#\s+(.+)$", text, re.MULTILINE)
    return m.group(1).strip() if m else fallback


def rewrite_links(body: str, source: Path, out_rel: Path) -> str:
    """Turns relative `.md` links into `.html` links inside the site and
    leaves everything else alone."""

    def repl(m: re.Match[str]) -> str:
        href = m.group(2)
        if href.startswith(("http://", "https://", "mailto:", "#")):
            return m.group(0)
        target, _, anchor = href.partition("#")
        if not target.endswith(".md"):
            resolved = (source.parent / target).resolve()
            try:
                rel_repo = resolved.relative_to(ROOT)
            except ValueError:
                return m.group(0)
            if rel_repo.parts and rel_repo.parts[0] == "assets":
                # Images live in the site's own assets directory.
                rel = os.path.relpath(Path("assets") / rel_repo.name, out_rel.parent)
                return f'{m.group(1)}"{rel.replace(os.sep, "/")}"'
            # Links to source files go to GitHub.
            return f'{m.group(1)}"{REPO_URL}/blob/HEAD/{rel_repo.as_posix()}"'
        resolved = (source.parent / target).resolve()
        try:
            rel_repo = resolved.relative_to(ROOT)
        except ValueError:
            return m.group(0)
        out_target = site_path_for(rel_repo)
        if out_target is None:
            return f'{m.group(1)}"{REPO_URL}/blob/HEAD/{rel_repo.as_posix()}"'
        rel = os.path.relpath(out_target, out_rel.parent).replace(os.sep, "/")
        if anchor:
            rel += "#" + anchor
        return f'{m.group(1)}"{rel}"'

    return re.sub(r'((?:href|src)=)"([^"]+)"', repl, body)


def site_path_for(rel_repo: Path) -> Path | None:
    text = rel_repo.as_posix()
    if text in TOP_LEVEL:
        return Path(TOP_LEVEL[text])
    if text.startswith("docs/") and text.endswith(".md"):
        out = Path(text).with_suffix(".html")
        if out.name == "README.html":
            out = out.with_name("index.html")
        return out
    return None


def render_markdown(source: Path, out_rel: Path, out_dir: Path) -> None:
    text = source.read_text(encoding="utf-8")
    title = first_heading(text, source.stem)
    body = markdown.markdown(
        text,
        extensions=["tables", "fenced_code", "toc", "sane_lists"],
        output_format="html",
    )
    body = rewrite_links(body, source, out_rel)
    depth = len(out_rel.parts) - 1
    page = layout(title, body, depth)
    target = out_dir / out_rel
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(page, encoding="utf-8")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="_site")
    args = ap.parse_args()
    out = Path(args.out).resolve()
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    # Static pages and assets.
    for item in SITE.iterdir():
        if item.name in ("build.py",):
            continue
        if item.is_dir():
            shutil.copytree(item, out / item.name)
        else:
            shutil.copy2(item, out / item.name)
    # Markdown documentation.
    count = 0
    for name, root in DOC_ROOTS.items():
        for source in sorted(root.rglob("*.md")):
            rel_repo = source.relative_to(ROOT)
            out_rel = site_path_for(rel_repo)
            if out_rel is None:
                continue
            render_markdown(source, out_rel, out)
            count += 1
    for name, target in TOP_LEVEL.items():
        source = ROOT / name
        if source.exists():
            render_markdown(source, Path(target), out)
            count += 1
    (out / ".nojekyll").write_text("")
    print(f"site written to {out} ({count} documentation pages)")


if __name__ == "__main__":
    main()
