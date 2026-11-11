# Theta website

A static site: hand-written HTML, one stylesheet, one small script. No framework, no build step,
no external requests (fonts, images and icons are all in this directory). All links are relative, so
it works under any base path (`/Quarry/` now, `/Theta/` after the rename).

```
site/
  index.html            landing page (hand-written)
  keybindings.html      generated from docs/KEYBINDINGS.md
  changelog.html        generated from CHANGELOG.md
  404.html              not-found page (repairs its own links when served from a nested URL)
  robots.txt, .nojekyll
  assets/
    css/site.css        design system (tokens documented at the top)
    js/site.js          theme toggle, single-key shortcuts (D, G), nav menu, tabs, OS-aware download hint, editor-mock animation, key search
    theta-mark.svg      generated: the theta mark, derived from packaging/icons/theta-mark.svg (do not edit)
    favicon.svg, favicon-32.png, apple-touch-icon.png, og.png
    fonts/              IBM Plex Serif, IBM Plex Sans and Lilex as WOFF2, with their OFL licences
    screens/            web images made from docs/screenshots (WebP 1600 + 900, PNG fallback)
  tools/                generators and dev helpers (below)
```

## Preview locally

```sh
cd site && python3 -m http.server 8000      # then open http://localhost:8000/
```

Any static file server will do; use one rather than opening the files with `file://`.

## Regenerate generated files

Everything below uses only the Python standard library (plus Pillow for images, Playwright for rasters).

| When | Command |
| --- | --- |
| `docs/KEYBINDINGS.md` or `CHANGELOG.md` changed, or the nav/footer partials changed | `python3 site/tools/build_pages.py` |
| a screenshot in `docs/screenshots/` was re-shot (same file names) | `python3 site/tools/optimize_screens.py` |
| the mark, the social card or the favicon changed | `node site/tools/render_assets.mjs` |
| the mark in `packaging/icons/` changed (after `packaging/icons/build.sh`) | `python3 site/tools/build_pages.py` (runs `mark.py`), then `node site/tools/render_assets.mjs` |

`build_pages.py` writes `keybindings.html` and `changelog.html`, and refreshes the nav and footer on
every page: edit `tools/partials/nav.html` and `tools/partials/footer.html`, not the copies in the pages.
Prose in the Markdown sources that says "Quarry" is shown as "Theta" (`RENAMES` in the script); code spans
and URLs are left alone. Re-run it after the rename lands in the sources.

`tools/check.mjs` (console errors, failed or external requests, links, image attributes, overflow at 360px,
tab order, page weight), `tools/shoot.mjs` and `tools/shoot_focus.mjs` (screenshots) are dev helpers that
need Playwright: `NODE_PATH=$(npm root -g) node site/tools/check.mjs http://localhost:8000`.

## Swapping things

- **Repository URL.** One authoritative copy for scripts: `REPO` at the top of `assets/js/site.js`, which sets the
  `href` of every link marked `data-repo="/path"`. The HTML keeps plain `href` fallbacks for visitors without
  JavaScript. After renaming the repository run
  `python3 site/tools/set_repo.py https://github.com/JesseWilliamson/Theta`, which rewrites the constant, the fallbacks
  and the GitHub Pages URL used by the absolute `og:image` tags (social crawlers need an absolute image URL).
  Then run `build_pages.py` if you also want regenerated pages.
- **The theta mark.** One source: `packaging/icons/gen_icon.py`, which draws the app icon, the in-app
  `theta-mark.svg` and everything below. `tools/mark.py` imports it and derives what the site draws: the hidden sprite
  `build_pages.py` puts in every page, with `<symbol id="theta">` (medium weight, for the nav, footer and eyebrow) and
  `<symbol id="theta-fine">` (the real GFS Didot theta, for the large marks), both used as `<use href="#theta"/>`
  in an `<svg>` sized by CSS; `assets/theta-mark.svg`; `assets/favicon.svg` (the 32 px pixel-grid app icon); and the
  mark in `tools/og.html`. `python3 site/tools/mark.py` rewrites them, `--check` (run by `check.mjs`) fails if they are
  stale. Never edit those files by hand. The marks are filled shapes, so there is no stroke width to tune; the two
  symbols exist because the real glyph's hairlines vanish below about 60 px tall.
- **Screenshots.** Pages refer to stable names in `assets/screens/` (`editor-results`, `completion-fk-joins`,
  `map`, `plan-insights`, `editable-grid`, `explorer-suggested-fks`, `theme-light`, `theme-picker`,
  `connection-ssh`, `vim-visual-block`, `keymap-choice`). Re-shoot into `docs/screenshots/` under the same
  numbered names and run `optimize_screens.py`. Update the alt text if the picture changes meaningfully.
- **Colours and type.** Custom properties at the top of `assets/css/site.css`, where the type scale is also documented.
  Headlines are IBM Plex Serif (Light Italic for the home hero, Light for page titles and h2, Regular for h3), prose is IBM
  Plex Sans, labels, key chips and code are Lilex. The editor mock deliberately keeps the app's own dark palette
  (`--ed-*`, `--syn-*`) in both page themes.
- **Fonts.** Self-hosted WOFF2 from the npm packages `@ibm/plex-serif`, `@ibm/plex-sans` and `@fontsource/lilex` (SIL OFL
  1.1; the licence texts sit next to the files). Plex is used in its per-script split files (`-Latin1`, `-Greek`, `-Pi`) with
  `unicode-range`, so a page downloads only the scripts it uses. To add or replace a face, `npm pack` the package in a
  temporary directory, copy the `.woff2` files, and add the `@font-face` rules to `site.css`. Each page preloads only the
  first serif face it paints (the italic on the home page, the roman elsewhere) and the Sans Regular.
- **Dark first.** The site is dark for every visitor, whatever `prefers-color-scheme` says; light is reachable only through
  the toggle, which saves `light` in `localStorage` (`theta-theme`). Enforced in three places: `site.css` has no
  `prefers-color-scheme` rule (dark values are the `:root` defaults, light lives under `[data-theme="light"]`);
  `tools/partials/assets.html` (injected into every page, and mirrored by hand in `404.html`) reads only that saved choice
  before first paint, so there is no flash; `site.js` does not consult the system either. `check.mjs` fails if a page is
  not dark under an emulated light preference.
- **Shortcuts.** A link with `data-key="d"` is followed when that letter is pressed outside a text field (`D` goes to
  `#download`, `G` to the repository). The chip drawn inside the button is only a hint.

## Deployment

`.github/workflows/pages.yml` publishes `site/` as it is to GitHub Pages on every push to `main` that touches
`site/**` or the workflow, and on manual dispatch. It has no build step.

GitHub Pages has to be enabled once for the repository: **Settings, Pages, Build and deployment, Source: GitHub
Actions**. (The workflow asks `actions/configure-pages` to enable it, but the default `GITHUB_TOKEN` often is not
allowed to; if the first run fails at that step, switch the setting by hand and re-run.) The site is then served at
`https://<owner>.github.io/<repo>/`.
