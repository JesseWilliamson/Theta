#!/usr/bin/env python3
"""Point the site at a renamed repository.

    python3 site/tools/set_repo.py https://github.com/JesseWilliamson/Theta

The repository URL has one authoritative copy for scripts (`REPO` in assets/js/site.js, which
rewrites every `data-repo` link at run time) and plain `href` fallbacks in the HTML for visitors
without JavaScript. This tool rewrites all of them, and the GitHub Pages URL (`https://<owner>.github.io/<repo>/`,
used by the absolute og:image tags, which social crawlers require), then you can re-run build_pages.py.

It reads the current URL from assets/js/site.js, so it can be run repeatedly.
"""
import re
import sys
from pathlib import Path

SITE = Path(__file__).resolve().parents[1]
TEXT = {".html", ".js", ".py", ".md", ".txt", ".css", ".svg"}


def pages_url(repo_url: str) -> str:
    m = re.match(r"^https://github\.com/([^/]+)/([^/]+?)/?$", repo_url)
    if not m:
        raise SystemExit(f"not a GitHub repository URL: {repo_url}")
    return f"https://{m.group(1).lower()}.github.io/{m.group(2)}/"


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    new = sys.argv[1].rstrip("/")
    js = (SITE / "assets" / "js" / "site.js").read_text()
    old = re.search(r'var REPO = "([^"]+)"', js).group(1)
    pairs = [(old, new), (pages_url(old), pages_url(new))]
    changed = 0
    for path in SITE.rglob("*"):
        if path.suffix not in TEXT or not path.is_file() or path.name in ("set_repo.py", "README.md"):
            continue
        text = path.read_text()
        out = text
        for a, b in pairs:
            out = out.replace(a, b)
        if out != text:
            path.write_text(out)
            changed += 1
            print("updated", path.relative_to(SITE))
    print(f"{old} -> {new}  ({changed} files)")


if __name__ == "__main__":
    main()
