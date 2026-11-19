# Browser tests (Playwright)

End-to-end tests of the real app: first-run setup, enrollment through the QR link, clocking in and
out of the zone, login, device revocation and security headers. They run against a real server
binary and a throwaway PostgreSQL database, with Chromium's virtual WebAuthn authenticator and
emulated geolocation. Headless only.

## Requirements

- Node 20+, the system Chromium at `/usr/bin/chromium-browser` (override with `E2E_CHROMIUM`).
  No Playwright browser download is needed.
- `dx` (Dioxus CLI) and a PostgreSQL server. `DATABASE_URL` comes from the environment or from the
  repository's `.env`; the database user must be allowed to `CREATE DATABASE`.

## Run

```sh
cd tests/e2e
npm ci
npx playwright test
```

The global setup (`harness/global-setup.ts`):

1. builds with `dx build --package app` (`CARGO_TARGET_DIR` defaults to
   `/srv/cache/cargo-build/tijdsregistratie`; set `E2E_SKIP_BUILD=1` to reuse the last build),
2. creates a database `tijd_e2e_<random>` and throwaway key files in a temp dir (mode 0600),
3. starts the server on `127.0.0.1:<free port>` and reads the `Setup code:` line from its output,
4. drops the database, removes the keys and stops the server afterwards.

Nothing secret is printed; the retained server log (`artifacts/server.log`) has the setup code redacted.

## Notes

- Scenarios share one installation and run in order in one file (`tests/e2e.spec.ts`).
- Screenshots of key screens go to `artifacts/` (git-ignored).
- The enrollment link is not shown as text, only as a QR code. The tests read it by rasterising the
  QR svg from the page and decoding it (jsQR), so they follow whatever link format the app produces.
- The dx debug build injects an `@import` of fonts.googleapis.com that the CSP blocks. It is ignored
  in the CSP assertion but listed in the console annotation of scenario 8.
