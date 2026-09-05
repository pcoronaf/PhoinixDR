#!/usr/bin/env python3
"""Builds the PhoinixDR site in English and Spanish.

- Static pages: site/*.html (English) and site/es/*.html (Spanish).
- Documentation: every Markdown file under docs/ (except docs/es/) is
  rendered to docs/<path>.html; the Spanish site renders docs/es/<path>.md
  to es/docs/<path>.html when it exists and otherwise falls back to the
  English source with a banner, so navigation is complete in both
  languages and pages can be translated one at a time.
- Top-level files (README, CHANGELOG, CONTRIBUTING, CODE_OF_CONDUCT,
  SECURITY) are rendered under docs/ in both languages (English content
  with the banner on the Spanish side).

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
DOCS = ROOT / "docs"
REPO_URL = "https://github.com/pcoronaf/PhoinixDR"

TOP_LEVEL = {
    "README.md": "readme.html",
    "CHANGELOG.md": "changelog.html",
    "CONTRIBUTING.md": "contributing.html",
    "CODE_OF_CONDUCT.md": "code-of-conduct.html",
    "SECURITY.md": "security.html",
}

LANGS = {
    "en": {
        "prefix": "",
        "html_lang": "en",
        "nav": [
            ("docs/index.html", "Documentation"),
            ("docs/user-guide/desktop.html", "User guide"),
            ("download.html", "Download"),
            ("community.html", "Community"),
            (REPO_URL, "GitHub"),
        ],
        "switch": "Español",
        "disclaimer": "PhoinixDR is provided “as is” and is used entirely at your own risk. Data recovery is inherently uncertain, and improper use may result in permanent data loss or damage. Always work from a copy or disk image when possible and recover files to a different storage device.",
        "read_disclaimer": "Read the disclaimer",
        "foot": "Open Source Data Recovery",
        "vibecoded": "Yes, PHOINIX is vibecoded",
        "origin": "Where PHOINIX came from",
        "untranslated": "",
    },
    "es": {
        "prefix": "es/",
        "html_lang": "es",
        "nav": [
            ("docs/index.html", "Documentación"),
            ("docs/user-guide/desktop.html", "Guía de usuario"),
            ("download.html", "Descargar"),
            ("community.html", "Comunidad"),
            (REPO_URL, "GitHub"),
        ],
        "switch": "English",
        "disclaimer": "PhoinixDR se proporciona «tal cual» y se utiliza enteramente bajo su propio riesgo. La recuperación de datos es incierta por naturaleza, y un uso inadecuado puede provocar la pérdida permanente de datos o daños. Siempre que sea posible, trabaje a partir de una copia o de una imagen de disco y recupere los archivos en un dispositivo de almacenamiento distinto.",
        "read_disclaimer": "Lea el aviso legal",
        "foot": "Recuperación de datos de código abierto",
        "vibecoded": "Sí, PHOINIX está «vibecodeado»",
        "origin": "De dónde viene PHOINIX",
        "untranslated": "Esta página aún no está traducida; se muestra la versión en inglés.",
    },
}


def rel_to(target: str, from_page: str) -> str:
    """Relative URL from the page at `from_page` (site path) to `target`."""
    if target.startswith(("http://", "https://")):
        return target
    rel = os.path.relpath(target, os.path.dirname(from_page) or ".")
    return rel.replace(os.sep, "/")


def layout(lang: str, title: str, body: str, page: str, counterpart: str, banner: str) -> str:
    L = LANGS[lang]
    p = L["prefix"]
    nav = "\n".join(
        f'    <a href="{rel_to(href if href.startswith("http") else p + href, page)}">{html.escape(label)}</a>'
        for href, label in L["nav"]
    )
    banner_html = f'<p class="banner">{html.escape(banner)}</p>\n' if banner else ""
    other = "es" if lang == "en" else "en"
    return f"""<!doctype html>
<html lang="{L["html_lang"]}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{html.escape(title)} · PhoinixDR</title>
<link rel="icon" href="{rel_to("assets/favicon.png", page)}">
<link rel="stylesheet" href="{rel_to("styles.css", page)}">
</head>
<body>
<header class="top">
  <a class="brand" href="{rel_to(p + "index.html", page)}"><img src="{rel_to("assets/logo-mark.png", page)}" alt="" width="28" height="28"> PhoinixDR</a>
  <nav>
{nav}
    <a class="lang" href="{rel_to(counterpart, page)}" hreflang="{other}">{L["switch"]}</a>
  </nav>
</header>
<main class="doc">
{banner_html}{body}
</main>
<footer class="foot">
  <p class="disclaimer">{html.escape(L["disclaimer"])} <a href="{rel_to(p + "docs/disclaimer.html", page)}">{L["read_disclaimer"]}</a>.</p>
  <p>PhoinixDR · {L["foot"]} · by <a href="https://github.com/pcoronaf">@pcoronaf</a> · MIT OR Apache-2.0 ·
  <a href="{rel_to(p + "docs/about/vibecoded.html", page)}">{L["vibecoded"]}</a> ·
  <a href="{rel_to(p + "docs/about/origin.html", page)}">{L["origin"]}</a></p>
</footer>
</body>
</html>
"""


IMAGE_SUFFIXES = {".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp"}


def first_heading(text: str, fallback: str) -> str:
    m = re.search(r"^#\s+(.+)$", text, re.MULTILINE)
    return m.group(1).strip() if m else fallback


def doc_key(rel_repo: Path) -> str | None:
    """The language-neutral document key (`user-guide/cli.md`,
    `readme.html`) of a repository path, or None when it is not rendered."""
    text = rel_repo.as_posix()
    if text in TOP_LEVEL:
        return TOP_LEVEL[text]
    if text.startswith("docs/es/") and text.endswith(".md"):
        return text[len("docs/es/") :]
    if text.startswith("docs/") and text.endswith(".md"):
        return text[len("docs/") :]
    return None


def site_path(key: str, lang: str) -> str:
    """Site path of a document key in a language."""
    p = LANGS[lang]["prefix"]
    if key.endswith(".html"):
        return f"{p}docs/{key}"
    out = Path(key).with_suffix(".html")
    if out.name == "README.html":
        out = out.with_name("index.html")
    return f"{p}docs/{out.as_posix()}"


def rewrite_links(body: str, source: Path, page: str, lang: str) -> str:
    """Turns relative links to Markdown files into site links in the same
    language, image links into site asset links, and other repository
    files into GitHub links."""

    def repl(m: re.Match[str]) -> str:
        href = m.group(2)
        if href.startswith(("http://", "https://", "mailto:", "#")):
            return m.group(0)
        target, _, anchor = href.partition("#")
        resolved = (source.parent / target).resolve()
        try:
            rel_repo = resolved.relative_to(ROOT)
        except ValueError:
            return m.group(0)
        if not target.endswith(".md"):
            if rel_repo.parts and rel_repo.parts[0] == "assets":
                return f'{m.group(1)}"{rel_to("assets/" + rel_repo.name, page)}"'
            if rel_repo.suffix.lower() in IMAGE_SUFFIXES and rel_repo.parts[:1] == ("docs",):
                # Images kept next to the documents (docs/**/images/*) are
                # copied once, language-independent, under /docs/.
                return f'{m.group(1)}"{rel_to(rel_repo.as_posix(), page)}"'
            return f'{m.group(1)}"{REPO_URL}/blob/HEAD/{rel_repo.as_posix()}"'
        key = doc_key(rel_repo)
        if key is None:
            return f'{m.group(1)}"{REPO_URL}/blob/HEAD/{rel_repo.as_posix()}"'
        link = rel_to(site_path(key, lang), page)
        if anchor:
            link += "#" + anchor
        return f'{m.group(1)}"{link}"'

    return re.sub(r'((?:href|src)=)"([^"]+)"', repl, body)


def render(source: Path, key: str, lang: str, out_dir: Path, fallback: bool) -> None:
    text = source.read_text(encoding="utf-8")
    title = first_heading(text, source.stem)
    body = markdown.markdown(
        text,
        extensions=["tables", "fenced_code", "toc", "sane_lists"],
        output_format="html",
    )
    page = site_path(key, lang)
    body = rewrite_links(body, source, page, lang)
    other = "es" if lang == "en" else "en"
    counterpart = site_path(key, other)
    banner = LANGS[lang]["untranslated"] if fallback else ""
    target = out_dir / page
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(layout(lang, title, body, page, counterpart, banner), encoding="utf-8")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="_site")
    args = ap.parse_args()
    out = Path(args.out).resolve()
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    # Static pages and assets (site/ → /, site/es/ → /es/).
    for item in SITE.iterdir():
        if item.name == "build.py":
            continue
        if item.is_dir():
            shutil.copytree(item, out / item.name)
        else:
            shutil.copy2(item, out / item.name)
    # Images referenced by the documents (docs/**/images/*), served once
    # under /docs/ for both languages.
    for image in sorted(DOCS.rglob("*")):
        if image.is_file() and image.suffix.lower() in IMAGE_SUFFIXES and "images" in image.relative_to(DOCS).parts:
            target = out / image.relative_to(ROOT)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(image, target)
    # Documents: every English document, rendered in both languages.
    sources: dict[str, Path] = {}
    for source in sorted(DOCS.rglob("*.md")):
        rel = source.relative_to(ROOT)
        if rel.as_posix().startswith("docs/es/"):
            continue
        key = doc_key(rel)
        if key is not None:
            sources[key] = source
    for name, key in TOP_LEVEL.items():
        if (ROOT / name).exists():
            sources[key] = ROOT / name
    # Spanish-only documents (none expected, but keep them reachable).
    for source in sorted((DOCS / "es").rglob("*.md")) if (DOCS / "es").exists() else []:
        key = doc_key(source.relative_to(ROOT))
        if key is not None and key not in sources:
            sources[key] = source
    count = 0
    for key, source in sources.items():
        english = source if not source.relative_to(ROOT).as_posix().startswith("docs/es/") else None
        spanish = DOCS / "es" / key if not key.endswith(".html") else None
        if english is not None:
            render(english, key, "en", out, fallback=False)
        if spanish is not None and spanish.exists():
            render(spanish, key, "es", out, fallback=False)
            if english is None:
                render(spanish, key, "en", out, fallback=False)
        elif english is not None:
            render(english, key, "es", out, fallback=True)
        count += 1
    (out / ".nojekyll").write_text("")
    print(f"site written to {out} ({count} documents in 2 languages)")


if __name__ == "__main__":
    main()
