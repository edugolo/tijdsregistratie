# Scalar API reference (vendored)

- File: `standalone.js`, from the npm package `@scalar/api-reference` version **1.83.0**
  (`dist/browser/standalone.js`, fetched once from `cdn.jsdelivr.net/npm/@scalar/api-reference@1.83.0/`).
- SHA-256: `dca741e8aac648751e3787945440eee5856ef11064c85753533168440c22f1c9`
- License: MIT, Copyright (c) Scalar. https://github.com/scalar/scalar/blob/main/LICENSE
- Used by `/api/v1/docs` (see `crates/app/src/server/api_v1/docs.rs`). The server embeds this file
  and serves it from `/api/v1/docs/scalar.js`, so the page needs no CDN and no CSP exception.
- To update: download the new version the same way, replace the file, update this note and check
  `/api/v1/docs` in a browser.
- The page configures Scalar (`data-configuration` in `docs.rs`) without its cloud features:
  `agent.disabled`, `mcp.disabled`, `showDeveloperTools: "never"`, `hideClientButton`,
  `hideTestRequestButton`, `telemetry: false`, `withDefaultFonts: false`, no `proxyUrl`. After an update
  check that the options still exist (grep the bundle) and that the end-to-end test (scenario 7i) passes.
