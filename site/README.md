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
    js/site.js          theme toggle, nav menu, tabs, OS-aware download hint, editor-mock animation, key search
    theta-mark.svg      the one source of the theta mark
    favicon.svg, favicon-32.png, apple-touch-icon.png, og.png
    fonts/              IBM Plex Sans and Lilex as WOFF, with their OFL licences
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
| fonts changed | `python3 site/tools/ttf2woff.py <fonts>.ttf --out site/assets/fonts` |

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
- **The theta mark.** `assets/theta-mark.svg` holds the geometry (`<g id="theta">`, 32 x 44 viewBox, `currentColor`).
  `build_pages.py` copies it into a hidden sprite on every page, which the nav, hero, footer and 404 draw with
  `<use href="#theta">`. Replace the contents of the group, keep the id and viewBox, then re-run the generators.
  `favicon.svg` has the same drawing with fixed colours on a dark tile; update it too and run `render_assets.mjs`.
- **Screenshots.** Pages refer to stable names in `assets/screens/` (`editor-results`, `completion-fk-joins`,
  `map`, `plan-insights`, `editable-grid`, `explorer-suggested-fks`, `theme-light`, `theme-picker`,
  `connection-ssh`, `vim-visual-block`, `keymap-choice`). Re-shoot into `docs/screenshots/` under the same
  numbered names and run `optimize_screens.py`. Update the alt text if the picture changes meaningfully.
- **Colours and type.** Custom properties at the top of `assets/css/site.css`. The editor mock deliberately keeps
  the app's own dark palette (`--ed-*`, `--syn-*`) in both page themes.

## Deployment

`.github/workflows/pages.yml` publishes `site/` as it is to GitHub Pages on every push to `main` that touches
`site/**` or the workflow, and on manual dispatch. It has no build step.

GitHub Pages has to be enabled once for the repository: **Settings, Pages, Build and deployment, Source: GitHub
Actions**. (The workflow asks `actions/configure-pages` to enable it, but the default `GITHUB_TOKEN` often is not
allowed to; if the first run fails at that step, switch the setting by hand and re-run.) The site is then served at
`https://<owner>.github.io/<repo>/`.
