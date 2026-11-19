import { Browser, BrowserContext, CDPSession, Page, Locator } from '@playwright/test';
import * as fs from 'node:fs';
import * as path from 'node:path';
import jsQR from 'jsqr';
import { PNG } from 'pngjs';

export const ARTIFACTS = path.resolve(__dirname, '../artifacts');

export const SCHOOL = { lat: 50.85, lng: 4.35 };
/** About 5 km north of the school. */
export const FAR_AWAY = { latitude: 50.895, longitude: 4.35, accuracy: 10 };
export const AT_SCHOOL = { latitude: SCHOOL.lat, longitude: SCHOOL.lng, accuracy: 10 };

export function origin(): string {
  const o = process.env.E2E_ORIGIN;
  if (!o) throw new Error('E2E_ORIGIN missing: global setup did not run');
  return o;
}

export interface Device {
  context: BrowserContext;
  page: Page;
  cdp: CDPSession;
  authenticatorId: string;
  /** Console messages of type error/warning, for the CSP and noise checks. */
  console: { type: string; text: string }[];
}

export interface DeviceOptions {
  mobile?: boolean;
  /** Undefined: no geolocation permission (a "denied" device). */
  geolocation?: { latitude: number; longitude: number; accuracy: number };
}

/** A fresh browser context with its own virtual authenticator: one person, one device. */
export async function newDevice(browser: Browser, opts: DeviceOptions = {}): Promise<Device> {
  const context = await browser.newContext({
    baseURL: origin(),
    ...(opts.mobile
      ? { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true, deviceScaleFactor: 2 }
      : { viewport: { width: 1100, height: 800 }, deviceScaleFactor: 2 }),
    locale: 'nl-BE',
    timezoneId: 'Europe/Brussels',
  });
  if (opts.geolocation) {
    await context.grantPermissions(['geolocation'], { origin: origin() });
    await context.setGeolocation(opts.geolocation);
  }
  const page = await context.newPage();
  const cdp = await context.newCDPSession(page);
  await cdp.send('WebAuthn.enable');
  const { authenticatorId } = await cdp.send('WebAuthn.addVirtualAuthenticator', {
    options: {
      protocol: 'ctap2',
      transport: 'internal',
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
      automaticPresenceSimulation: true,
    },
  });
  const consoleMessages: Device['console'] = [];
  page.on('console', (m) => {
    if (m.type() === 'error' || m.type() === 'warning') {
      consoleMessages.push({ type: m.type(), text: m.text() });
    }
  });
  page.on('pageerror', (e) => consoleMessages.push({ type: 'pageerror', text: e.message }));
  return { context, page, cdp, authenticatorId, console: consoleMessages };
}

export async function shot(page: Page, name: string): Promise<void> {
  fs.mkdirSync(ARTIFACTS, { recursive: true });
  await page.screenshot({ path: path.join(ARTIFACTS, `${name}.png`), fullPage: true });
}

/**
 * Decodes the QR code panel. The page shows only the QR image (no link as text), so the link is
 * read from the inline SVG: its dark modules are rasterised here and decoded with jsQR. A
 * screenshot of the panel is not used because the SVG is currently clipped by its container.
 */
export async function readQrLink(qr: Locator): Promise<string> {
  const svg = await qr.innerHTML();
  const size = Number(/viewBox="0 0 (\d+) \d+"/.exec(svg)?.[1]);
  if (!size) throw new Error('QR svg has no viewBox');
  const scale = 4;
  const png = new PNG({ width: size * scale, height: size * scale });
  png.data.fill(255);
  const dark = svg.slice(svg.indexOf('fill="#111111"'));
  for (const m of dark.matchAll(/M(\d+) (\d+)h(\d+)v(\d+)h-\d+z/g)) {
    const [x, y, w, h] = m.slice(1).map(Number);
    for (let py = y * scale; py < (y + h) * scale; py++) {
      for (let px = x * scale; px < (x + w) * scale; px++) {
        const o = (py * png.width + px) * 4;
        png.data[o] = png.data[o + 1] = png.data[o + 2] = 0;
      }
    }
  }
  const result = jsQR(new Uint8ClampedArray(png.data), png.width, png.height);
  if (!result) throw new Error('QR code could not be decoded');
  return result.data;
}

/** True when the QR svg fits inside its container (otherwise the code is cut off and unscannable). */
export async function qrFitsContainer(qr: Locator): Promise<boolean> {
  const box = await qr.boundingBox();
  const svgBox = await qr.locator('svg').boundingBox();
  if (!box || !svgBox) return false;
  return svgBox.x + svgBox.width <= box.x + box.width + 1 && svgBox.y + svgBox.height <= box.y + box.height + 1;
}
