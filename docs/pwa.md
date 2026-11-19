# PWA notes

Static PWA files live in `public/` and are copied to the web root by `dx` (`public_dir` in
`crates/app/Dioxus.toml`, relative to the crate). The head tags and service worker registration
come from the `PwaHead` component in `crates/app/src/client/pwa.rs`.

| File | Purpose |
| --- | --- |
| `manifest.webmanifest` | App identity, colours, icons |
| `sw.js` | Service worker |
| `icon.svg` | Icon source (brand colour `#1e3a8a`, glyph inside the 80% safe zone) |
| `icon-192.png`, `icon-512.png`, `icon-maskable-512.png`, `apple-touch-icon.png`, `favicon.ico` | Rendered from `icon.svg` |

## Branding per deployment

The manifest is static for now. An operator can edit `name`, `short_name`, `theme_color` and
`background_color` in `public/manifest.webmanifest` (and the `theme-color` meta in `pwa.rs`) before
building. The server may template it later.

## Service worker

- Navigation: network first, falls back to the cached `/` when offline.
- Same-origin static assets (`/assets/`, `/wasm/`, images, js, css): cache first, refreshed in the background.
- `/api/` requests, non-GET requests and `/sw.js` are never cached.
- To invalidate caches (for example after changing the shell), bump `CACHE_VERSION` in `public/sw.js`.
  On activate the worker deletes every older `tijdsregistratie-*` cache.

## Installing

- Android (Chrome): installs as a WebAPK; the app can capture `beforeinstallprompt` for its own button.
- iOS (Safari): no prompt. Share, then "Zet op beginscherm". Only Safari can install; the app shows the steps.

## Passkeys need the final domain

Passkeys are bound to the domain (RP ID). Pick the final domain before enrolling real users;
changing it invalidates every passkey. Service workers and install also require HTTPS
(`localhost` is exempt for development).

## Regenerating the icons

Edit `public/icon.svg`, then render each size with headless Chromium, for example:

```sh
for n in 512 192 180 48; do
  echo "<html><body style='margin:0'><img src='file://$PWD/public/icon.svg' width=$n height=$n style='display:block'></body></html>" > /tmp/r$n.html
  chromium-browser --headless --no-sandbox --hide-scrollbars --force-device-scale-factor=1 \
    --window-size=$n,$n --screenshot=/tmp/o$n.png file:///tmp/r$n.html
done
```

Copy `o512` to `icon-512.png` and `icon-maskable-512.png`, `o192` to `icon-192.png`, `o180` to
`apple-touch-icon.png`; wrap `o48` in a one-image ICO container for `favicon.ico`.
