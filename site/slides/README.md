# ado-aw intro slides

A short (~3–4 min, 7-slide) intro deck for **ado-aw — Continuous AI for Azure
DevOps**, built with [Slidev](https://sli.dev). Markdown-driven, mirroring the
docs-site brand palette. It lives inside the docs site (`site/slides/`) and is
published alongside the docs at **`/ado-aw/slides/`**.

The narrative: CI/CD is the rung we already trust → Continuous AI is the next
rung (async, triggered, runs on existing runners) → `ado-aw` is the platform →
its key principles → how the three-stage security model works.

## Present locally

```bash
cd site/slides
npm install      # first time only
npm run dev      # opens the deck at http://localhost:3030
```

Press `f` for fullscreen, `o` for slide overview, and `p` for presenter mode
(speaker notes live in `<!-- ... -->` blocks in `slides.md`).

## How it ships on the site

The deck is part of the docs build. From `site/`:

```bash
npm run build        # builds the deck into public/slides/, then runs astro build
```

`npm run build:slides` runs `slidev build --base /ado-aw/slides/ --out
../public/slides`, so the static deck (presenter view included) is copied into
the published site. Live cross-device remote control needs the dev server, so
that one feature is only available via `npm run dev`.

The generated `public/slides/` output is git-ignored — it is regenerated on
every build.

## Export to PDF

```bash
cd site/slides
npm run export   # writes dist/ado-aw-intro.pdf
```

> PDF export uses `playwright-chromium` (a devDependency, installed by a plain
> `npm install`). The first export after the process is idle can occasionally
> produce an incomplete file due to a dev-server warm-up race — just run it
> again.

## Editing

All content is in [`slides.md`](./slides.md); brand styling is in
[`style.css`](./style.css). Slides are separated by `---`; per-slide layout is
set in each slide's frontmatter.
