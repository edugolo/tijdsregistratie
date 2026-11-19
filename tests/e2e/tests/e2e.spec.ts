import { expect, test, Browser, Page } from '@playwright/test';
import { Client } from 'pg';
import {
  AT_SCHOOL,
  Device,
  FAR_AWAY,
  SCHOOL,
  newDevice,
  origin,
  qrFitsContainer,
  readQrLink,
  shot,
} from '../harness/device';

test.describe.configure({ mode: 'default' });

const SETUP_ORGANISATION = 'Voorbeeldschool Test';
const EMPLOYEE_PAGE_TIMEOUT = 20_000;

let admin: Device;
let fatima: Device;
const early: Device[] = [];
/** Whether the QR svg fitted its container, one entry per enrollment (asserted in the last test). */
const qrFits: boolean[] = [];

async function countEvents(): Promise<number> {
  const db = new Client({ connectionString: process.env.E2E_DB_URL });
  await db.connect();
  try {
    const res = await db.query('SELECT count(*)::int AS n FROM time_event');
    return res.rows[0].n as number;
  } finally {
    await db.end();
  }
}

/** The team row of one person on the manager page. */
function row(page: Page, name: string) {
  return page.getByRole('listitem').filter({ hasText: name });
}

async function openTeam(page: Page) {
  await page.goto('/beheer');
  await expect(page.getByRole('heading', { name: 'Mijn team' })).toBeVisible();
}

/** Adds an employee, links a device through the QR link and walks the employee onboarding. */
async function enrollEmployee(
  browser: Browser,
  fullName: string,
  geolocation: { latitude: number; longitude: number; accuracy: number } | undefined,
): Promise<Device> {
  const page = admin.page;
  await openTeam(page);
  await page.getByLabel('Naam van de medewerker').fill(fullName);
  await page.getByRole('button', { name: 'Toevoegen' }).click();
  const personRow = row(page, fullName);
  await expect(personRow).toBeVisible();
  await personRow.getByRole('button', { name: 'Koppel toestel' }).click();

  const qr = page.getByRole('img', { name: 'QR-code om het toestel te koppelen' });
  await expect(qr).toBeVisible();
  qrFits.push(await qrFitsContainer(qr));
  const link = await readQrLink(qr);
  if (fullName === 'Fatima Test') await shot(page, '02-admin-enroll-qr');

  // Employee phone: no geolocation permission yet, so the explanation screen is shown.
  const phone = await newDevice(browser, { mobile: true });
  await phone.page.goto(link);
  const first = fullName.split(' ')[0];
  await expect(phone.page.getByRole('heading', { name: `Welkom ${first}` })).toBeVisible({
    timeout: EMPLOYEE_PAGE_TIMEOUT,
  });
  if (fullName === 'Fatima Test') await shot(phone.page, '03-employee-welcome');
  await phone.page.getByRole('button', { name: 'Verder' }).click();
  await expect(phone.page.getByRole('heading', { name: 'Maak je sleutel aan' })).toBeVisible();
  await phone.page.getByRole('button', { name: 'Sleutel aanmaken' }).click();
  await expect(phone.page.getByRole('heading', { name: 'Waarom je locatie?' })).toBeVisible();
  if (fullName === 'Fatima Test') await shot(phone.page, '04-employee-location-explain');

  if (geolocation) {
    await phone.context.grantPermissions(['geolocation'], { origin: origin() });
    await phone.context.setGeolocation(geolocation);
  }
  await phone.page.getByRole('button', { name: 'Ik begrijp het' }).click();
  await expect(phone.page.getByRole('button', { name: 'Start je dag' })).toBeVisible({
    timeout: EMPLOYEE_PAGE_TIMEOUT,
  });

  // The manager page polls and flips to "Gekoppeld".
  await expect(page.getByText('Gekoppeld').first()).toBeVisible({ timeout: 15_000 });
  await page.getByRole('button', { name: 'Klaar' }).click();
  await expect(row(page, fullName).getByText('Gekoppeld')).toBeVisible();
  return phone;
}

test.afterAll(async () => {
  for (const d of [admin, fatima, ...early]) await d?.context.close().catch(() => undefined);
});

test('1. first-run setup creates the organisation and the admin', async ({ browser }) => {
  admin = await newDevice(browser);
  const { page } = admin;

  await page.goto('/');
  await expect(page).toHaveURL(/\/setup$/);
  await expect(page.getByRole('heading', { name: 'Welkom' })).toBeVisible();
  await shot(page, '01a-setup-welcome');

  // Wrong code first: refused.
  await page.getByLabel('Installatiecode').fill('XXXX-XXXX-XXXX');
  await page.getByRole('button', { name: 'Volgende' }).click();
  await expect(page.getByRole('alert')).toBeVisible();

  await page.getByLabel('Installatiecode').fill(process.env.E2E_SETUP_CODE!);
  await page.getByRole('button', { name: 'Volgende' }).click();

  await expect(page.getByRole('heading', { name: 'Je organisatie' })).toBeVisible();
  await page.getByLabel('Naam van de organisatie').fill(SETUP_ORGANISATION);
  await page.getByLabel('Korte naam').fill('Voorbeeld');
  await shot(page, '01b-setup-org');
  await page.getByRole('button', { name: 'Volgende' }).click();

  await expect(page.getByRole('heading', { name: 'Je eerste school' })).toBeVisible();
  await page.getByLabel('Naam van de school', { exact: true }).fill('Basisschool Voorbeeld');
  await page.getByLabel('Breedtegraad').fill(String(SCHOOL.lat));
  await page.getByLabel('Lengtegraad').fill(String(SCHOOL.lng));
  await page.getByLabel('Straal in meter').fill('150');
  await shot(page, '01c-setup-school');
  await page.getByRole('button', { name: 'Volgende' }).click();

  await expect(page.getByRole('heading', { name: 'Over jou' })).toBeVisible();
  await page.getByLabel('Je naam').fill('Beheerder Test');
  await expect(page.getByLabel('Ik ben ook verantwoordelijke van deze school')).toBeChecked();
  await page.getByRole('button', { name: 'Volgende' }).click();

  await expect(page.getByRole('heading', { name: 'Maak je sleutel aan' })).toBeVisible();
  await shot(page, '01d-setup-key');
  await page.getByRole('button', { name: 'Sleutel aanmaken' }).click();
  await expect(page.getByRole('heading', { name: 'Klaar' })).toBeVisible();
  await shot(page, '01e-setup-done');

  expect(cspViolations(admin)).toEqual([]);
});

test('2. admin adds Fatima, links her phone through the enrollment link', async ({ browser }) => {
  await page2Overview();
  fatima = await enrollEmployee(browser, 'Fatima Test', AT_SCHOOL);
  await shot(fatima.page, '05-employee-home');
  await shot(admin.page, '06-admin-team-linked');
  expect(cspViolations(fatima)).toEqual([]);

  async function page2Overview() {
    await admin.page.getByRole('link', { name: 'Naar het overzicht' }).click();
    await expect(admin.page).toHaveURL(/\/beheer$/);
    await expect(admin.page.getByRole('heading', { name: 'Mijn team' })).toBeVisible();
  }
});

test('3. employee in the zone: one START event, then STOP gives the day summary', async () => {
  const { page } = fatima;
  const before = await countEvents();
  await page.getByRole('button', { name: 'Start je dag' }).dblclick();
  await expect(page.getByText(/Gestart om \d{2}:\d{2}/)).toBeVisible();
  await shot(page, '07-employee-working');
  // Give a possible second request time to land before counting.
  await expect.poll(countEvents, { timeout: 5000 }).toBe(before + 1);
  await page.waitForTimeout(1500);
  expect(await countEvents()).toBe(before + 1);

  await openTeam(admin.page);
  await expect(row(admin.page, 'Fatima Test').getByText(/Gestart/)).toBeVisible();

  await page.getByRole('button', { name: 'Stop je dag' }).click();
  await expect(page.getByRole('heading', { name: /Gestopt om \d{2}:\d{2}/ })).toBeVisible();
  await expect(page.getByText('Tot morgen!')).toBeVisible();
  await shot(page, '08-employee-day-summary');
  expect(await countEvents()).toBe(before + 2);
  expect(cspViolations(fatima)).toEqual([]);

  await openTeam(admin.page);
  await expect(row(admin.page, 'Fatima Test').getByText(/Gestopt/)).toBeVisible();
});

test('4. employee out of the zone: start is kept and flagged', async ({ browser }) => {
  const jonas = await enrollEmployee(browser, 'Jonas Test', FAR_AWAY);
  early.push(jonas);
  await jonas.page.getByRole('button', { name: 'Start je dag' }).click();
  await expect(jonas.page.getByRole('heading', { name: 'Je bent niet op school' })).toBeVisible();
  await expect(jonas.page.getByText(/We hebben je start toch bewaard om/)).toBeVisible();
  await shot(jonas.page, '09-employee-out-of-zone');

  await openTeam(admin.page);
  await expect(row(admin.page, 'Jonas Test').getByText(/Niet op school/)).toBeVisible();
  await shot(admin.page, '10-admin-team-out-of-zone');
});

test('5. location denied: start is kept, "we weten niet waar je bent"', async ({ browser }) => {
  const amira = await enrollEmployee(browser, 'Amira Test', undefined);
  early.push(amira);
  const before = await countEvents();
  await amira.page.getByRole('button', { name: 'Start je dag' }).click();
  await expect(
    amira.page.getByRole('heading', { name: 'We weten niet waar je bent' }),
  ).toBeVisible({ timeout: 20_000 });
  await shot(amira.page, '11-employee-no-location');
  expect(await countEvents()).toBe(before + 1);
});

test('6. login with the passkey after the session is gone', async () => {
  const { page, context } = fatima;
  await context.clearCookies();
  await page.goto('/');
  await expect(page).toHaveURL(/\/aanmelden$/);
  await shot(page, '12-employee-login');
  await page.getByRole('button', { name: 'Aanmelden' }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByRole('heading', { name: /Gestopt om|Goeiemorgen/ })).toBeVisible();
});

test('7. revoking the devices ends the session and the old passkey stops working', async () => {
  const { page } = fatima;
  await openTeam(admin.page);
  const personRow = row(admin.page, 'Fatima Test');
  await personRow.getByRole('button', { name: 'Toestel intrekken' }).click();
  await personRow.getByRole('button', { name: 'Ja' }).click();
  await expect(row(admin.page, 'Fatima Test').getByText('Geen toestel')).toBeVisible();

  // The employee's next request ends on the login screen.
  await page.goto('/');
  await expect(page).toHaveURL(/\/aanmelden$/);
  await page.getByRole('button', { name: 'Aanmelden' }).click();
  await expect(page.getByText('Er ging iets mis. Probeer opnieuw.')).toBeVisible();
  await expect(page).toHaveURL(/\/aanmelden$/);
  await shot(page, '13-employee-login-refused');
});

test('8. security headers and no CSP violations', async ({ request }) => {
  const res = await request.get(origin() + '/');
  const h = res.headers();
  expect(h['content-security-policy'], 'Content-Security-Policy').toBeTruthy();
  expect(h['x-frame-options'], 'X-Frame-Options').toBeTruthy();
  expect(h['referrer-policy'], 'Referrer-Policy').toBeTruthy();
  expect(h['x-content-type-options']).toBe('nosniff');

  // List all console errors and warnings seen on every device, for the report.
  const all = [admin, fatima, ...early].flatMap((d) => d?.console ?? []);
  test.info().annotations.push({
    type: 'console',
    description: JSON.stringify([...new Set(all.map((m) => `${m.type}: ${m.text}`))], null, 1),
  });
  for (const d of [admin, fatima, ...early]) expect(cspViolations(d)).toEqual([]);
});

/**
 * CSP violations seen in the console. The `@import` of fonts.googleapis.com comes from the dx
 * debug-build toast template (dx dev tooling, not from the app) and is ignored here; it is still
 * listed in the console annotation of scenario 8.
 */
function cspViolations(d: Device): string[] {
  return d.console
    .filter((m) => !m.text.includes('fonts.googleapis.com'))
    .filter((m) => /Content Security Policy|Refused to (load|apply|execute|connect)/i.test(m.text))
    .map((m) => m.text);
}

// Kept last and separate so a layout bug does not block the functional scenarios above.
test('9. the enrollment QR code is not cropped by its container', async () => {
  expect(qrFits.length).toBeGreaterThan(0);
  expect(qrFits, 'QR svg must fit inside its container, otherwise it cannot be scanned').not.toContain(false);
});
