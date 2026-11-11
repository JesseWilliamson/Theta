#!/usr/bin/env python3
"""Generate keybindings.html and changelog.html from the repository's Markdown, and keep the
shared nav / footer identical on every page.

    python3 site/tools/build_pages.py            # from anywhere inside the repo

Inputs : docs/KEYBINDINGS.md, CHANGELOG.md, site/tools/partials/{nav,footer}.html
Outputs: site/keybindings.html, site/changelog.html (overwritten), and the regions between
         <!--@nav--> ... <!--/@nav--> and <!--@footer--> ... <!--/@footer--> in index.html,
         404.html and the two generated pages.

Only the Python standard library is used. The Markdown handled is the small subset these two
files use: headings, paragraphs, bullet lists (nested), pipe tables, inline code, bold, italics
and links.
"""
from __future__ import annotations

import html
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SITE = ROOT / "site"
PARTIALS = SITE / "tools" / "partials"

# Kept in step with assets/js/site.js and the HTML fallbacks by tools/set_repo.py.
REPO = "https://github.com/JesseWilliamson/Quarry"
PAGES_URL = "https://jessewilliamson.github.io/Quarry/"

# The product is called Theta; the Markdown sources may still say the old name in prose.
# Code spans, URLs and file paths are left alone: they name things that exist in the repository.
RENAMES = [(re.compile(r"\bQuarry\b"), "Theta")]

# Phrases in docs/KEYBINDINGS.md that are internal API names rather than user-facing text.
SOURCE_FIXES = [
    ("`Effect::RequestCompletion`", "open completion"),
    ("(`Effect::FormatSelection`; the host formats the selected lines)", "(formats the selected lines)"),
    ("`Save` + `CloseTab` / `CloseTab`", "save and close the tab / close the tab"),
    ("and `Effect::Scroll` asks the host to scroll", "and the view scrolls to match"),
    ("(`Effect::SetClipboard` to write; ", "(written on yank; "),
    ("Anything else becomes `Effect::HostCommand(text)`.", "Anything else is passed to the application as a command."),
]


# ---------------------------------------------------------------- inline markdown

def rename(text: str) -> str:
    for pat, new in RENAMES:
        text = pat.sub(new, text)
    return text


CURRENT_DOC = "docs/KEYBINDINGS.md"  # the Markdown file being converted; set by the builders


def link_href(url: str) -> str:
    """Relative links in the Markdown point into the repository, so resolve them against it."""
    if url.startswith("#"):  # an anchor in the source document, which only exists on GitHub
        return f"{REPO}/blob/main/{CURRENT_DOC}{url}"
    if re.match(r"^(https?:|mailto:)", url):
        return url
    return f"{REPO}/blob/main/{url}"


def inline(text: str) -> str:
    """Markdown inline -> HTML. Code spans are cut out first so nothing inside them is touched."""
    codes: list[str] = []

    def stash(m: re.Match) -> str:
        body = m.group(2)
        if body.startswith(" ") and body.endswith(" ") and len(body) > 2:
            body = body[1:-1]
        codes.append(f"<code>{html.escape(body, quote=False)}</code>")
        return f"\x00{len(codes) - 1}\x00"

    text = re.sub(r"(`+)(.+?)\1", stash, text)
    text = html.escape(rename(text), quote=False)

    def a(m: re.Match) -> str:
        url = html.unescape(m.group(2))
        href = link_href(url)
        rel = ""
        if href.startswith(REPO):
            rel = f' data-repo="{html.escape(href[len(REPO):], quote=True)}"'
        return f'<a href="{html.escape(href, quote=True)}"{rel}>{m.group(1)}</a>'

    text = re.sub(r"\[([^\]]+)\]\(([^)\s]+)\)", a, text)
    text = re.sub(r"\*\*(.+?)\*\*", r"<strong>\1</strong>", text)
    text = re.sub(r"(?<![\w*])\*(?!\s)(.+?)(?<!\s)\*(?![\w*])", r"<em>\1</em>", text)
    return re.sub(r"\x00(\d+)\x00", lambda m: codes[int(m.group(1))], text)


def key_cell(text: str) -> str:
    """First column of a key table: every code span becomes a chip; a span such as `h j k l` becomes four."""
    out, pos = [], 0
    for m in re.finditer(r"(`+)(.+?)\1", text):
        between = text[pos : m.start()]
        if between.strip():
            out.append(f'<span class="alt">{html.escape(rename(between.strip()), quote=False)}</span>')
        body = m.group(2)
        if body.startswith(" ") and body.endswith(" ") and len(body) > 2:
            body = body[1:-1]
        parts = body.split() if " " in body.strip() else [body]
        out.append("".join(f"<kbd>{html.escape(p, quote=False)}</kbd>" for p in parts))
        pos = m.end()
    tail = text[pos:].strip()
    if tail:
        out.append(f'<span class="alt">{html.escape(rename(tail), quote=False)}</span>')
    return '<span class="keys">' + " ".join(out) + "</span>"


# ---------------------------------------------------------------- block markdown

Block = tuple  # ("p", text) | ("ul", [(level, text)]) | ("table", header, rows) | ("quote", text)


def split_row(line: str) -> list[str]:
    line = line.strip().strip("|")
    # `\|` inside a cell is an escaped pipe; cells never contain a bare one inside code spans here.
    return [c.strip().replace("\\|", "|") for c in re.split(r"(?<!\\)\|", line)]


def parse_blocks(lines: list[str]) -> list[Block]:
    blocks: list[Block] = []
    i = 0
    while i < len(lines):
        line = lines[i]
        if not line.strip():
            i += 1
        elif line.lstrip().startswith("|") and i + 1 < len(lines) and re.match(r"^\s*\|[\s:|-]+\|\s*$", lines[i + 1]):
            header = split_row(line)
            i += 2
            rows = []
            while i < len(lines) and lines[i].lstrip().startswith("|"):
                rows.append(split_row(lines[i]))
                i += 1
            blocks.append(("table", header, rows))
        elif re.match(r"^\s*[-*] ", line):
            items: list[list] = []
            while i < len(lines):
                cur = lines[i]
                m = re.match(r"^(\s*)[-*] (.*)$", cur)
                if m:
                    items.append([len(m.group(1)) // 2, m.group(2)])
                elif cur.strip() and cur.startswith(" ") and items:
                    items[-1][1] += " " + cur.strip()  # continuation line of the previous item
                elif not cur.strip() and i + 1 < len(lines) and re.match(r"^\s+\S", lines[i + 1]) and items:
                    pass  # blank line inside a list item
                else:
                    break
                i += 1
            blocks.append(("ul", [(lv, tx) for lv, tx in items]))
        elif line.startswith(">"):
            buf = []
            while i < len(lines) and lines[i].startswith(">"):
                buf.append(lines[i].lstrip("> ").rstrip())
                i += 1
            blocks.append(("quote", " ".join(buf)))
        else:
            buf = []
            while i < len(lines) and lines[i].strip() and not lines[i].lstrip().startswith(("|", ">")) and not re.match(r"^\s*[-*] ", lines[i]):
                buf.append(lines[i].strip())
                i += 1
            blocks.append(("p", " ".join(buf)))
    return blocks


def render_list(items: list[tuple[int, str]]) -> str:
    out, stack = [], 0
    for lv, text in items:
        lv = min(lv, stack + 1) if out else 0
        while stack > lv:
            out.append("</li></ul>")
            stack -= 1
        if stack == lv and out and not out[-1].endswith("<ul>"):
            out.append("</li>")
        if lv > stack:
            out.append("<ul>")
            stack += 1
        elif not out:
            out.append("<ul>")
            stack = 0
        out.append(f"<li>{inline(text)}")
    out.append("</li></ul>" * (stack + 1))
    return "".join(out).replace("<ul><ul>", "<ul>")


# ---------------------------------------------------------------- shared page pieces

def partial(name: str, current: str) -> str:
    text = (PARTIALS / name).read_text()
    if "{{MARK}}" in text:
        # One source for the mark: assets/theta-mark.svg. It is inlined into each page's hidden sprite so
        # <use href="#theta"> needs no extra request and works from any URL depth (404.html).
        mark = re.search(r'<g id="theta"\s[^>]*>.*?</g>', (SITE / "assets" / "theta-mark.svg").read_text(), flags=re.S).group(0)
        text = text.replace("{{MARK}}", re.sub(r"\s*\n\s*", "", mark))
    if name == "nav.html":
        text = text.replace(f'<a href="{current}"', f'<a aria-current="page" href="{current}"')
    return text.rstrip("\n")


def head(title: str, description: str, page: str) -> str:
    t, d = html.escape(title), html.escape(description, quote=True)
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{t}</title>
<meta name="description" content="{d}">
<meta name="theme-color" content="#181b1f">
<meta property="og:type" content="website">
<meta property="og:site_name" content="Theta">
<meta property="og:title" content="{t}">
<meta property="og:description" content="{d}">
<meta property="og:image" content="{PAGES_URL}assets/og.png">
<meta property="og:image:width" content="1200">
<meta property="og:image:height" content="630">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:title" content="{t}">
<meta name="twitter:description" content="{d}">
<meta name="twitter:image" content="{PAGES_URL}assets/og.png">
<link rel="icon" href="assets/favicon.svg" type="image/svg+xml">
<link rel="icon" href="assets/favicon-32.png" type="image/png" sizes="32x32">
<link rel="apple-touch-icon" href="assets/apple-touch-icon.png">
<link rel="preload" href="assets/fonts/IBMPlexSans-Regular.woff" as="font" type="font/woff" crossorigin>
<link rel="preload" href="assets/fonts/IBMPlexSans-SemiBold.woff" as="font" type="font/woff" crossorigin>
<link rel="stylesheet" href="assets/css/site.css">
<script>
  // Runs before first paint so the saved theme never flashes. Storage can throw (private windows), hence try/catch.
  (function () {{
    var d = document.documentElement;
    d.classList.add("js");
    try {{ var t = localStorage.getItem("theta-theme"); if (t === "light" || t === "dark") d.setAttribute("data-theme", t); }} catch (e) {{}}
  }})();
</script>
</head>
<body>
<!--@nav-->
<!--/@nav-->
"""


def page_end() -> str:
    return """
<!--@footer-->
<!--/@footer-->
<script src="assets/js/site.js" defer></script>
</body>
</html>
"""


def inject_partials(path: Path) -> None:
    """Replace the content between the nav / footer markers with the partials."""
    text = path.read_text()
    for tag, name in (("nav", "nav.html"), ("footer", "footer.html")):
        body = partial(name, path.name)
        text, n = re.subn(rf"<!--@{tag}-->.*?<!--/@{tag}-->", lambda m: f"<!--@{tag}-->\n{body}\n<!--/@{tag}-->", text, flags=re.S)
        if n != 1:
            raise SystemExit(f"{path.name}: expected one <!--@{tag}--> region, found {n}")
    path.write_text(text)


# ---------------------------------------------------------------- keybindings

def split_sections(md: str, level: int) -> list[tuple[str, list[str]]]:
    """Split on headings of exactly `level` (e.g. 2 for '## '). Returns [(title, body_lines)]; text before the first is title ''."""
    marker = "#" * level + " "
    out: list[tuple[str, list[str]]] = [("", [])]
    for line in md.splitlines():
        if line.startswith(marker):
            out.append((line[len(marker):].strip(), []))
        else:
            out[-1][1].append(line)
    return out


def table_html(header: list[str], rows: list[list[str]]) -> str:
    # The wide "who wins a key" style tables are not key lists; only 2-column key tables are rendered here.
    head_cells = "".join(f"<th scope='col'>{html.escape(h)}</th>" for h in header)
    body = []
    for r in rows:
        cells = [key_cell(r[0])] + [inline(c) for c in r[1:]]
        body.append("<tr data-row>" + "".join(f"<td>{c}</td>" for c in cells) + "</tr>")
    return f'<div class="table-scroll"><table class="keys-table"><thead><tr>{head_cells}</tr></thead><tbody>{"".join(body)}</tbody></table></div>'


def group_html(title: str, blocks: list[Block], note: str | None = None) -> str:
    parts = [f'<section class="block"><h3 class="group">{html.escape(rename(title))}</h3>']
    if note:
        parts.append(f'<p class="group-note">{note}</p>')
    for b in blocks:
        if b[0] == "table":
            parts.append(table_html(b[1], b[2]))
        elif b[0] == "p":
            parts.append(f'<p class="group-note">{inline(b[1])}</p>')
        elif b[0] == "ul":
            parts.append(f'<div class="prose">{render_list(b[1])}</div>')
    parts.append("</section>")
    return "".join(parts)


def build_keybindings() -> str:
    global CURRENT_DOC
    CURRENT_DOC = "docs/KEYBINDINGS.md"
    md = (ROOT / "docs" / "KEYBINDINGS.md").read_text()
    for old, new in SOURCE_FIXES:
        md = md.replace(old, new)
    h2 = {t: body for t, body in split_sections(md, 2)}

    # Application tab: every H2 except the editor, with its tables. "Who wins a key" is prose-heavy, so it stays on GitHub.
    app_groups = []
    titles = {"Application (global)": "Global", "Pickers and the command palette": "Pickers and the command palette"}
    for title, body in split_sections(md, 2)[1:]:
        if title == "Editor":
            continue
        top = body
        for sub_title, sub_body in split_sections("\n".join(body), 3)[:1]:
            top = sub_body
        blocks = [b for b in parse_blocks(top) if b[0] == "table" or (b[0] == "p" and len(b[1]) < 400 and title != "Application (global)") or (b[0] == "p" and title == "Application (global)" and b[1].startswith("These actions"))]
        blocks = [b for b in blocks if b[0] != "p" or "Defined in" not in b[1]]
        if any(b[0] == "table" for b in blocks):
            app_groups.append(group_html(titles.get(title, title), blocks))

    editor = split_sections("\n".join(h2["Editor"]), 3)
    maps = {t: body for t, body in editor}
    panels = {}
    helix_space = None
    for key, src in (("helix", "Helix key map"), ("vim", "Vim key map"), ("standard", "Standard key map")):
        subs = split_sections("\n".join(maps[src]), 4)
        groups = []
        intro_blocks = parse_blocks(subs[0][1])
        for sub_title, sub_body in subs[1:]:
            blocks = parse_blocks(sub_body)
            if key == "vim" and sub_title.startswith("Space menu") and helix_space:
                groups.append(group_html("Space menu", helix_space, "The same which-key menu as in the Helix keymap."))
                continue
            if sub_title == "Space menu" and key == "helix":
                helix_space = [b for b in blocks if b[0] == "table"]
            if any(b[0] == "table" for b in blocks) or sub_title in ("Registers", "Search"):
                groups.append(group_html(sub_title, blocks))
        # Standard has its table directly under the heading.
        direct = [b for b in intro_blocks if b[0] == "table"]
        if direct:
            groups.insert(0, group_html("Editing", direct))
        panels[key] = groups
    panels["application"] = app_groups

    notes = {
        "helix": "The default keymap. Selection first: choose what to act on, then act. Counts work wherever Helix takes one; in Select mode every motion extends.",
        "vim": "Verb, then motion: <code>[count]{operator}[count]{motion or text object}</code>. <kbd>space</kbd> is the leader. Neovim wins where it differs from Vim. Not in scope: Vimscript, <code>:map</code>, windows, folds, digraphs.",
        "standard": "Not modal, and follows Zed&rsquo;s default bindings. <kbd>ctrl</kbd> and <kbd>cmd</kbd> are interchangeable, and shift extends any motion.",
        "application": "Chords active everywhere except where a focused view owns the keystroke. While the SQL editor has focus, the editing engine&rsquo;s keys for the active keymap win; see <a href=\"%s/blob/main/docs/KEYBINDINGS.md\" data-repo=\"/blob/main/docs/KEYBINDINGS.md\">Who wins a key</a>." % REPO,
    }
    labels = [("helix", "Helix"), ("vim", "Vim"), ("standard", "Standard"), ("application", "Application")]

    tabs = "".join(
        f'<button class="tab" role="tab" id="tab-{k}" aria-controls="panel-{k}" aria-selected="{"true" if i == 0 else "false"}"'
        f'{"" if i == 0 else " tabindex=\"-1\""}>{lab}<span class="count" aria-hidden="true"></span></button>'
        for i, (k, lab) in enumerate(labels)
    )
    panel_html = "".join(
        f'<div class="tabpanel" role="tabpanel" id="panel-{k}" aria-labelledby="tab-{k}" tabindex="0">'
        f'<h2 class="panel-title js-hide">{lab}</h2><p class="panel-note">{notes[k]}</p>{"".join(panels[k])}'
        f'<p class="no-match">Nothing in {lab} matches that search.</p></div>'
        for k, lab in labels
    )
    total = sum(len(re.findall(r"data-row", "".join(v))) for v in panels.values())

    return (
        head("Key bindings | Theta", "Every key binding in Theta for the Helix, Vim and Standard keymaps, plus application shortcuts. Searchable.", "keybindings.html")
        + f"""
<main id="main">
  <section class="page-head">
    <div class="wrap">
      <p class="eyebrow"><svg class="m" viewBox="0 0 32 44" aria-hidden="true"><use href="#theta"/></svg> Docs</p>
      <h1>Key bindings</h1>
      <p>Three keymaps and the application shortcuts, {total} bindings in all. <kbd>mod</kbd> is <kbd>cmd</kbd> on macOS and <kbd>ctrl</kbd> on Linux and Windows. In the app, Settings &rsaquo; Keymap lists the same keys with their actions.</p>
    </div>
  </section>
  <div class="ref" data-tabs data-hash>
    <div class="ref-bar">
      <div class="wrap">
        <div class="tablist" role="tablist" aria-label="Keymap">{tabs}</div>
        <div class="js-only" style="align-items:center;gap:14px;flex:1 1 260px;justify-content:flex-end">
          <span class="search-count" id="match-count" aria-live="polite"></span>
          <div class="search"><svg aria-hidden="true"><use href="#i-search"/></svg><input id="q" type="search" placeholder="Search keys and actions" aria-label="Search key bindings" autocomplete="off" spellcheck="false"><kbd aria-hidden="true">/</kbd></div>
        </div>
      </div>
    </div>
    <div class="wrap">
      {panel_html}
      <p class="intro">Generated from <a href="{REPO}/blob/main/docs/KEYBINDINGS.md" data-repo="/blob/main/docs/KEYBINDINGS.md">docs/KEYBINDINGS.md</a> by <code>site/tools/build_pages.py</code>. The Markdown file is the source of truth.</p>
    </div>
  </div>
</main>
"""
        + page_end()
    )


# ---------------------------------------------------------------- changelog

def build_changelog() -> str:
    global CURRENT_DOC
    CURRENT_DOC = "CHANGELOG.md"
    md = (ROOT / "CHANGELOG.md").read_text()
    compare = dict(re.findall(r"^\[([^\]]+)\]: (\S+)$", md, flags=re.M))
    releases = []
    for title, body in split_sections(md, 2)[1:]:
        m = re.match(r"\[([^\]]+)\](?: - (\d{4}-\d{2}-\d{2}))?", title)
        if not m:
            continue
        ver, date = m.group(1), m.group(2)
        body_lines = [ln for ln in body if not re.match(r"^\[[^\]]+\]: ", ln)]
        if not "".join(body_lines).strip():
            continue  # an empty [Unreleased] heading says nothing
        parts = []
        for sub_title, sub_body in split_sections("\n".join(body_lines), 3):
            blocks = parse_blocks(sub_body)
            if sub_title:
                parts.append(f"<h3>{html.escape(sub_title)}</h3>")
            for b in blocks:
                if b[0] == "p":
                    parts.append(f"<p>{inline(b[1])}</p>")
                elif b[0] == "ul":
                    parts.append(render_list(b[1]))
                elif b[0] == "quote":
                    parts.append(f"<p>{inline(b[1])}</p>")
        link = compare.get(ver, "")
        link_html = ""
        if link.startswith(REPO):
            link_html = f'<a class="mono" style="font-size:.8125rem" href="{html.escape(link)}" data-repo="{html.escape(link[len(REPO):])}">Compare on GitHub</a>'
        slug = "v" + re.sub(r"[^0-9a-z]+", "-", ver.lower()).strip("-")
        shown = html.escape(ver)
        releases.append(
            f'<article class="release" id="{slug}"><div class="ver"><h2>{shown}</h2>'
            + (f"<time datetime=\"{date}\">{date}</time>" if date else "")
            + f"{link_html}</div><div class=\"prose\">{''.join(parts)}</div></article>"
        )
    return (
        head("Changelog | Theta", "What changed in each Theta release, newest first. Generated from CHANGELOG.md.", "changelog.html")
        + f"""
<main id="main">
  <section class="page-head">
    <div class="wrap">
      <p class="eyebrow"><svg class="m" viewBox="0 0 32 44" aria-hidden="true"><use href="#theta"/></svg> Docs</p>
      <h1>Changelog</h1>
      <p>Every release, newest first. Theta follows <a href="https://keepachangelog.com/en/1.1.0/">Keep a Changelog</a> and intends to follow Semantic Versioning once releases begin.</p>
    </div>
  </section>
  <section class="ref">
    <div class="wrap">
      {''.join(releases)}
      <p class="intro">Generated from <a href="{REPO}/blob/main/CHANGELOG.md" data-repo="/blob/main/CHANGELOG.md">CHANGELOG.md</a> by <code>site/tools/build_pages.py</code>.</p>
    </div>
  </section>
</main>
"""
        + page_end()
    )


def main() -> None:
    (SITE / "keybindings.html").write_text(build_keybindings())
    (SITE / "changelog.html").write_text(build_changelog())
    for name in ("index.html", "404.html", "keybindings.html", "changelog.html"):
        if (SITE / name).exists():
            inject_partials(SITE / name)
    print("wrote keybindings.html, changelog.html; refreshed nav/footer")


if __name__ == "__main__":
    main()
