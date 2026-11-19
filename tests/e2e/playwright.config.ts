import { defineConfig } from '@playwright/test';

// System Chromium; Playwright's own browser download is not used.
const executablePath = process.env.E2E_CHROMIUM ?? '/usr/bin/chromium-browser';

export default defineConfig({
  testDir: './tests',
  outputDir: './artifacts/test-results',
  globalSetup: './harness/global-setup.ts',
  // The scenarios share one server, one database and one installation: strictly sequential.
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 60_000,
  expect: { timeout: 15_000 },
  reporter: [['list']],
  use: {
    headless: true,
    locale: 'nl-BE',
    timezoneId: 'Europe/Brussels',
    launchOptions: {
      executablePath,
      args: ['--no-sandbox', '--disable-gpu'],
    },
  },
});
