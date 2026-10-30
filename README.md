# Klipolo

Klipolo is a Mac video editor you talk to. Drop in raw footage, say what you
want in a chat box, and it edits. Anything that removes footage comes back as a
proposal you approve, with the removed parts shown in red on the timeline.

It runs on Apple Silicon and Intel Macs and is free while in beta.

## Repo layout

```
site/            Landing page (static, hand-written, no build step)
  index.html     The whole page
  styles.css     All styles, light and dark mode
  assets/        Icon, favicon and self-hosted fonts
```

## Preview the site locally

```sh
python3 -m http.server -d site
```

Then open http://localhost:8000.

## Download links

The two download URLs (Apple Silicon and Intel) live in one place: the
`DOWNLOAD LINKS` comment in `site/index.html`. Until the first build ships they
point to the `#coming-nov-4` section. When the .dmg files are ready, swap the
two `href` values and remove that section.

## Fonts

Archivo is self-hosted from `site/assets/fonts/` under the SIL Open Font
License (see `OFL.txt` next to the font file).
