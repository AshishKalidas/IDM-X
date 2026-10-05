# IDMX website

A static site: no build step, no dependencies. Open `index.html` or serve the folder
(`python3 -m http.server`) and upload it anywhere (Netlify, Cloudflare Pages, S3, your own server).

* `index.html` — the page. Release notes and download entries live in the `release-data` JSON block at the bottom.
* `assets/site.css`, `assets/site.js`, `assets/tokens.css` (same design tokens as the app).
* `assets/shots/` — real screenshots of the app (dark and light).
* `downloads/` — put the installers here with the names listed in `release-data`
  (e.g. `IDMX-0.1.0-macos-arm64.dmg`). Each button checks that its file exists and shows
  "Not published yet" until it does, so nothing 404s for visitors.

To publish a new version: add the files to `downloads/`, bump `latest` and the file names, and add an entry at the top of `releases`.
