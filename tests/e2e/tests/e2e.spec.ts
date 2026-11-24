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

/** The row of one person on the Vandaag or Mensen page. */
function row(page: Page, name: string) {
  return page.getByRole('listitem').filter({ hasText: name });
}

async function openToday(page: Page) {
  await page.goto('/beheer');
  await expect(page.getByRole('heading', { name: 'Vandaag', exact: true })).toBeVisible();
}

async function openPeople(page: Page) {
  await page.goto('/beheer/mensen');
  await expect(page.getByRole('heading', { name: 'Mensen', exact: true })).toBeVisible();
}

/** Adds an employee, links a device through the QR link and walks the employee onboarding. */
async function enrollEmployee(
  browser: Browser,
  fullName: string,
  geolocation: { latitude: number; longitude: number; accuracy: number } | undefined,
): Promise<Device> {
  const page = admin.page;
  await openPeople(page);
  await page.getByRole('button', { name: 'Persoon toevoegen' }).click();
  await page.getByLabel('Naam', { exact: true }).fill(fullName);
  await page.getByRole('button', { name: 'Toevoegen', exact: true }).click();
  const personRow = row(page, fullName);
  await expect(personRow).toBeVisible();
  // The row opens the detail panel, which holds "Toestel koppelen".
  await personRow.getByRole('button').click();
  await page.getByRole('button', { name: 'Toestel koppelen' }).click();

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
    await expect(admin.page.getByRole('heading', { name: 'Vandaag', exact: true })).toBeVisible();
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

  await openToday(admin.page);
  await expect(row(admin.page, 'Fatima Test').getByText('Gestart', { exact: true })).toBeVisible();

  await page.getByRole('button', { name: 'Stop je dag' }).click();
  await expect(page.getByRole('heading', { name: /Gestopt om \d{2}:\d{2}/ })).toBeVisible();
  await expect(page.getByText('Tot morgen!')).toBeVisible();
  await shot(page, '08-employee-day-summary');
  expect(await countEvents()).toBe(before + 2);
  expect(cspViolations(fatima)).toEqual([]);

  // Someone who stopped is in no list of Vandaag.
  await openToday(admin.page);
  await expect(row(admin.page, 'Fatima Test')).toHaveCount(0);
});

test('4. employee out of the zone: start is kept and flagged', async ({ browser }) => {
  const jonas = await enrollEmployee(browser, 'Jonas Test', FAR_AWAY);
  early.push(jonas);
  await jonas.page.getByRole('button', { name: 'Start je dag' }).click();
  await expect(jonas.page.getByRole('heading', { name: 'Je bent niet op school' })).toBeVisible();
  await expect(jonas.page.getByText(/We hebben je start toch bewaard om/)).toBeVisible();
  await shot(jonas.page, '09-employee-out-of-zone');

  await openToday(admin.page);
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
  await openPeople(admin.page);
  await row(admin.page, 'Fatima Test').getByRole('button').click();
  await admin.page.getByRole('button', { name: 'Toestel intrekken' }).click();
  await admin.page.getByRole('button', { name: 'Ja', exact: true }).click();
  await expect(row(admin.page, 'Fatima Test').getByText('Geen toestel')).toBeVisible();

  // The employee's next request ends on the login screen.
  await page.goto('/');
  await expect(page).toHaveURL(/\/aanmelden$/);
  await page.getByRole('button', { name: 'Aanmelden' }).click();
  await expect(page.getByText('Aanmelden is niet gelukt. Probeer opnieuw.')).toBeVisible();
  await expect(page).toHaveURL(/\/aanmelden$/);
  await shot(page, '13-employee-login-refused');
});

test('7b. admin adds and edits a school in Instellingen', async () => {
  const { page } = admin;
  await page.goto('/beheer/instellingen');
  await page.getByRole('button', { name: 'Scholen', exact: true }).click();
  await page.getByRole('button', { name: 'School toevoegen' }).click();
  await expect(page.getByRole('heading', { name: 'Nieuwe school' })).toBeVisible();

  // An untouched new form has defaults and shows no errors.
  await expect(page.getByLabel('Straal (meter)')).toHaveValue('150');
  await expect(page.getByLabel('Pauze (minuten)')).toHaveValue('30');
  await expect(page.getByLabel('Pauze na (minuten werk)')).toHaveValue('360');
  await expect(page.getByRole('alert')).toHaveCount(0);

  // Trying to save shows what is missing.
  await page.getByRole('button', { name: 'Opslaan' }).click();
  await expect(page.getByText('Vul een naam in.')).toBeVisible();

  await page.getByLabel('Naam', { exact: true }).fill('Tweede School');
  await page.getByLabel('Adres').fill('Voorbeeldlaan 2');
  await page.getByLabel('Breedtegraad').fill('50,86');
  await page.getByLabel('Lengtegraad').fill('4.36');
  await page.getByRole('button', { name: 'Opslaan' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Opgeslagen' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Nieuwe school' })).toHaveCount(0);
  await expect(row(page, 'Tweede School')).toBeVisible();
  await shot(page, '14-admin-sites-desktop');
  await page.setViewportSize({ width: 390, height: 844 });
  await shot(page, '15-admin-sites-390');
  await page.setViewportSize({ width: 1280, height: 720 });

  // Edit opens that school; switching to a new one replaces the form.
  await page.getByRole('button', { name: 'Bewerken: Tweede School' }).click();
  await expect(page.getByRole('heading', { name: 'School bewerken: Tweede School' })).toBeVisible();
  await expect(page.getByLabel('Adres')).toHaveValue('Voorbeeldlaan 2');
  await page.getByRole('button', { name: 'School toevoegen' }).click();
  await expect(page.getByRole('heading', { name: 'Nieuwe school' })).toBeVisible();
  await expect(page.getByLabel('Naam', { exact: true })).toHaveValue('');

  // Unsaved changes are only discarded after confirmation.
  await page.getByRole('button', { name: 'Bewerken: Tweede School' }).click();
  await page.getByLabel('Naam', { exact: true }).fill('Tweede School bis');
  await page.getByRole('button', { name: 'School toevoegen' }).click();
  await expect(page.getByRole('button', { name: 'Wijzigingen weggooien' })).toBeVisible();
  await page.getByRole('button', { name: 'Verder bewerken' }).click();
  await expect(page.getByLabel('Naam', { exact: true })).toHaveValue('Tweede School bis');
  await page.getByRole('button', { name: 'Opslaan' }).click();
  await expect(row(page, 'Tweede School bis')).toBeVisible();
  await expect(page.getByRole('heading', { name: /School bewerken/ })).toHaveCount(0);
});

test('7c. admin adds an HR person (personeelsdienst) without a school', async () => {
  const { page } = admin;
  await openPeople(page);
  await page.getByRole('button', { name: 'Persoon toevoegen' }).click();

  // The validation message sits outside the label: the field keeps its plain accessible name.
  await page.getByRole('button', { name: 'Toevoegen', exact: true }).click();
  await expect(page.getByRole('alert').filter({ hasText: 'Vul een naam in.' })).toBeVisible();
  await expect(page.getByLabel('Naam', { exact: true })).toHaveAttribute('aria-invalid', 'true');

  await page.getByLabel('Naam', { exact: true }).fill('Hanne Personeel');
  await expect(page.getByLabel('School', { exact: true })).toBeVisible();
  await page.locator('label.st-seg', { hasText: 'Personeelsdienst' }).click();
  // HR works for the whole organisation: no school to choose.
  await expect(page.getByLabel('School', { exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Toevoegen', exact: true }).click();

  const hr = row(page, 'Hanne Personeel');
  await expect(hr).toBeVisible();
  await expect(hr).toContainText('Personeelsdienst');
  await expect(hr).not.toContainText('Basisschool Voorbeeld');
  const panel = page.locator('aside.st-panel');
  await expect(panel.getByRole('heading', { name: 'Hanne Personeel' })).toBeVisible();
  await expect(panel).toContainText('Personeelsdienst');
  await expect(panel.locator('header p')).not.toContainText('·');

  // An admin who is also manager of a school shows "Beheerder" only, not the school.
  await row(page, 'Beheerder Test').getByRole('button').click();
  await expect(panel.locator('header p')).toHaveText('Beheerder');
});

test('7d. admin links a second device in Mijn toestellen, both sign in, then one is revoked', async ({
  browser,
}) => {
  const { page } = admin;
  await page.goto('/beheer/instellingen');
  await page.getByRole('button', { name: 'Mijn toestellen', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Mijn toestellen' })).toBeVisible();
  const devices = page.locator('li.st-row');
  await expect(devices).toHaveCount(1);
  await expect(devices.first()).toContainText('Dit toestel');
  await shot(page, '16-admin-my-devices');

  await page.getByRole('button', { name: 'Nog een toestel koppelen', exact: true }).click();
  const qr = page.getByRole('img', { name: 'QR-code om het toestel te koppelen' });
  await expect(qr).toBeVisible();
  const link = await readQrLink(qr);

  // A second computer with its own virtual authenticator registers an additional passkey.
  const second = await newDevice(browser);
  early.push(second);
  await second.page.goto(link);
  await expect(second.page.getByRole('heading', { name: 'Welkom Beheerder' })).toBeVisible({
    timeout: EMPLOYEE_PAGE_TIMEOUT,
  });
  await second.page.getByRole('button', { name: 'Verder' }).click();
  await second.page.getByRole('button', { name: 'Sleutel aanmaken' }).click();
  await expect(second.page).toHaveURL(/\/beheer/, { timeout: EMPLOYEE_PAGE_TIMEOUT });
  await expect(page.getByText('Gekoppeld').first()).toBeVisible({ timeout: 15_000 });
  await page.getByRole('button', { name: 'Klaar' }).click();

  // The first device stays signed in and now lists both devices.
  await expect(devices).toHaveCount(2);
  await shot(page, '17-admin-two-devices');

  // Both devices can sign in with their own passkey.
  for (const d of [second, admin]) {
    await d.context.clearCookies();
    await d.page.goto('/');
    await expect(d.page).toHaveURL(/\/aanmelden$/);
    await d.page.getByRole('button', { name: 'Aanmelden' }).click();
    await expect(d.page).toHaveURL(/\/beheer/, { timeout: EMPLOYEE_PAGE_TIMEOUT });
  }

  // Revoke the other device: only its sessions end.
  await page.goto('/beheer/instellingen');
  await page.getByRole('button', { name: 'Mijn toestellen', exact: true }).click();
  await expect(devices).toHaveCount(2);
  const other = devices.filter({ hasNotText: 'Dit toestel' });
  await other.getByRole('button', { name: 'Intrekken' }).click();
  await other.getByRole('button', { name: 'Ja', exact: true }).click();
  await expect(devices).toHaveCount(1);
  await expect(devices.first()).toContainText('Dit toestel');

  await second.page.goto('/beheer');
  await expect(second.page).toHaveURL(/\/aanmelden$/);
  await second.page.getByRole('button', { name: 'Aanmelden' }).click();
  await expect(second.page.getByText('Aanmelden is niet gelukt. Probeer opnieuw.')).toBeVisible();
  // The first device is still signed in after a reload.
  await page.reload();
  await page.getByRole('button', { name: 'Mijn toestellen', exact: true }).click();
  await expect(devices).toHaveCount(1);
});

/** Hours and minutes of "now" in the browser time zone, as `<input type="time">` writes them. */
async function nowClock(page: Page): Promise<string> {
  return page.evaluate(() =>
    new Intl.DateTimeFormat('en-GB', { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).format(new Date()),
  );
}

test('7e. admin follows up the hours: correct a day, validate the period, a correction is refused, reopen', async () => {
  const { page } = admin;
  await page.goto('/beheer/uren');
  await expect(page.getByRole('heading', { name: 'Uren', exact: true })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Uren', exact: true })).toHaveAttribute('aria-current', 'page');

  // This week: Fatima has her start and stop of today.
  const card = (name: string) => page.locator('section.st-hours-person').filter({ hasText: name });
  await expect(card('Fatima Test')).toBeVisible();
  await expect(card('Fatima Test').locator('tbody tr')).toHaveCount(1);
  await expect(card('Jonas Test').getByText('Niet op school')).toBeVisible();
  await shot(page, '18-admin-hours-desktop');

  // The validation bar belongs to one school.
  await expect(page.getByText('Kies een school om de periode te valideren.')).toBeVisible();
  await page.getByRole('button', { name: 'Basisschool Voorbeeld', exact: true }).click();
  const bar = page.getByRole('group', { name: /Validatie van/ });
  await expect(bar).toContainText('Niet gevalideerd');

  // Amira started without a location and never stopped: the admin adds the missing stop.
  await card('Amira Test').locator('button.st-link').click();
  const panel = page.locator('aside.st-panel');
  await expect(panel.getByRole('heading', { name: 'Amira Test' })).toBeVisible();
  await expect(panel.getByText('Geen locatie')).toBeVisible();
  await panel.getByRole('button', { name: 'Ontbrekende start/stop toevoegen' }).click();
  await panel.getByLabel('Tijdstip').fill(await nowClock(page));
  await panel.getByRole('button', { name: 'Opslaan' }).click();
  await expect(panel.getByText('Geef een reden van 3 tot 500 tekens.')).toBeVisible();
  await panel.getByLabel('Reden').fill('Vergat te stoppen');
  await panel.getByRole('button', { name: 'Opslaan' }).click();
  await expect(panel.getByText('Vergat te stoppen')).toBeVisible();
  await expect(panel.getByText('Beheerder Test, ')).toBeVisible();
  await expect(panel.getByText('Gecorrigeerd')).toBeVisible();
  await shot(page, '19-admin-hours-day-panel');
  await page.setViewportSize({ width: 390, height: 844 });
  await shot(page, '20-admin-hours-day-panel-390');
  await panel.getByRole('button', { name: 'Sluiten' }).click();
  await shot(page, '21-admin-hours-390');
  await page.setViewportSize({ width: 1280, height: 720 });
  await expect(card('Amira Test').getByText('Gecorrigeerd')).toBeVisible();

  // Validate the period: the confirmation lists the open points. The day panel stays open.
  const [dayResponse] = await Promise.all([
    page.waitForResponse((r) => r.url().endsWith('/api/hours/day')),
    card('Amira Test').locator('button.st-link').click(),
  ]);
  const day = await dayResponse.json();
  await expect(panel.getByRole('button', { name: 'Tijd aanpassen' }).first()).toBeVisible();
  await bar.getByRole('button', { name: 'Periode valideren' }).click();
  await expect(bar.getByText('1 dag niet op school')).toBeVisible();
  await bar.getByRole('button', { name: 'Valideren', exact: true }).click();
  await expect(bar).toContainText('Gevalideerd door Beheerder Test op');
  await expect(card('Amira Test').getByText('Gevalideerd', { exact: true })).toBeVisible();
  await shot(page, '22-admin-hours-validated');

  // The open day panel locks at once: the notice shows and no edit button is left.
  await expect(panel.getByText('Deze periode is gevalideerd. Heropen ze eerst.')).toBeVisible();
  await expect(panel.getByRole('button', { name: 'Tijd aanpassen' })).toHaveCount(0);
  await expect(panel.getByRole('button', { name: 'Ongeldig maken' })).toHaveCount(0);
  await expect(panel.getByRole('button', { name: 'Ontbrekende start/stop toevoegen' })).toHaveCount(0);

  // Whatever the screen shows, the server refuses a further correction.
  const refused = await page.request.post('/api/hours/correct', {
    headers: { Origin: origin() },
    data: {
      person_id: day.person_id,
      date: day.date,
      change: { Void: { event_id: day.effective[0].key } },
      reason: 'Nog een aanpassing',
    },
  });
  expect(refused.status()).toBe(409);
  await panel.getByRole('button', { name: 'Sluiten' }).click();

  // Reopen: validated no more.
  await bar.getByRole('button', { name: 'Heropenen' }).click();
  await bar.getByRole('button', { name: 'Ja', exact: true }).click();
  await expect(bar).toContainText('Niet gevalideerd');
  await expect(bar.getByRole('button', { name: 'Periode valideren' })).toBeVisible();

  // An open day panel unlocks at once as well.
  await card('Amira Test').locator('button.st-link').click();
  await expect(panel.getByRole('button', { name: 'Tijd aanpassen' }).first()).toBeVisible();
  await expect(panel.getByText('Deze periode is gevalideerd. Heropen ze eerst.')).toHaveCount(0);
  await panel.getByRole('button', { name: 'Sluiten' }).click();
});

test('7f. admin sets contract, payroll number and cost centre', async () => {
  const { page } = admin;
  await openPeople(page);
  await row(page, 'Fatima Test').getByRole('button').click();
  const panel = page.locator('aside.st-panel');
  await expect(panel.getByRole('heading', { name: 'Contract' })).toBeVisible();
  await panel.locator('#contract-day-0').fill('7:36');
  await panel.locator('#contract-day-1').fill('x');
  await panel.getByRole('button', { name: 'Contract opslaan' }).click();
  await expect(panel.getByText('Gebruik uren en minuten, bijvoorbeeld 7:36.')).toBeVisible();
  await panel.locator('#contract-day-1').fill('7:36');
  await panel.getByRole('button', { name: 'Contract opslaan' }).click();
  await expect(panel.getByText('Opgeslagen')).toBeVisible();

  // The payroll number is masked until asked for.
  const payroll = panel.locator('#payroll-number');
  await expect(payroll).toHaveAttribute('type', 'password');
  await payroll.fill('P-100');
  await panel.getByRole('button', { name: 'Personeelsnummer opslaan' }).click();
  await expect(panel.getByText('Opgeslagen')).toBeVisible();
  await openPeople(page);
  await row(page, 'Fatima Test').getByRole('button').click();
  await expect(panel.locator('#payroll-number')).toHaveAttribute('type', 'password');
  await expect(panel.locator('#contract-day-0')).toHaveValue('7:36');
  await panel.getByRole('button', { name: 'Toon personeelsnummer' }).click();
  await expect(panel.locator('#payroll-number')).toHaveValue('P-100');
  await expect(panel.locator('#payroll-number')).toHaveAttribute('type', 'text');
  await shot(page, '23-admin-person-contract');

  // Taken by someone else.
  await row(page, 'Amira Test').getByRole('button').click();
  await panel.locator('#payroll-number').fill('p-100');
  await panel.getByRole('button', { name: 'Personeelsnummer opslaan' }).click();
  await expect(panel.getByText('Dit personeelsnummer is al in gebruik.')).toBeVisible();

  // Cost centre of a school.
  await page.goto('/beheer/instellingen');
  await page.getByRole('button', { name: 'Scholen', exact: true }).click();
  await page.getByRole('button', { name: 'Bewerken: Basisschool Voorbeeld' }).click();
  await page.getByLabel('Kostenplaats').fill('KP-1');
  await page.getByRole('button', { name: 'Opslaan' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Opgeslagen' })).toBeVisible();
  await page.getByRole('button', { name: 'Bewerken: Basisschool Voorbeeld' }).click();
  await expect(page.getByLabel('Kostenplaats')).toHaveValue('KP-1');
});

/** Today in the organisation time zone as `<input type="date">` writes it. */
async function orgToday(page: Page): Promise<string> {
  return page.evaluate(() =>
    new Intl.DateTimeFormat('en-CA', { timeZone: 'Europe/Brussels', year: 'numeric', month: '2-digit', day: '2-digit' }).format(
      new Date(),
    ),
  );
}

/** Opens Uren > Export and chooses today as the period. */
async function openExportForToday(page: Page) {
  await page.goto('/beheer/uren');
  await page.getByRole('button', { name: 'Export', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Export voor de loonadministratie' })).toBeVisible();
  await page.getByRole('button', { name: 'Andere periode', exact: true }).click();
  const today = await orgToday(page);
  await page.locator('#export-from').fill(today);
  await page.locator('#export-to').fill(today);
}

test('7g. HR or admin exports: problems first, then preview and download, then verify', async () => {
  const { page } = admin;
  await openExportForToday(page);
  const download = page.getByRole('button', { name: 'Downloaden', exact: true });
  const preview = page.getByRole('button', { name: 'Voorbeeld', exact: true });
  await expect(download).toBeDisabled();
  await preview.click();
  const problems = page.getByRole('alert').filter({ hasText: 'Dit moet eerst nog in orde' });
  await expect(problems).toContainText('nog niet gevalideerd');
  await expect(download).toBeDisabled();
  await shot(page, '24-admin-export-problems');

  // Each unvalidated school links to Uren for that school and period: validate it there.
  const links = await problems.getByRole('button', { name: 'Naar de uren' }).count();
  expect(links).toBeGreaterThan(0);
  for (let i = 0; i < links; i++) {
    await openExportForToday(page);
    await preview.click();
    await problems.getByRole('button', { name: 'Naar de uren' }).first().click();
    const bar = page.getByRole('group', { name: /Validatie van/ });
    await expect(bar).toBeVisible();
    await bar.getByRole('button', { name: 'Periode valideren' }).click();
    await bar.getByRole('button', { name: 'Valideren', exact: true }).click();
    await expect(bar).toContainText('Gevalideerd door Beheerder Test op');
  }

  // Now only missing payroll numbers are left: each name opens the person panel.
  await openExportForToday(page);
  await preview.click();
  await expect(problems).toContainText('nog geen personeelsnummer');
  await expect(problems).not.toContainText('nog niet gevalideerd');
  const names = await problems.locator('ul.st-issues').last().getByRole('button').allTextContents();
  expect(names.length).toBeGreaterThan(0);
  for (const [index, name] of names.entries()) {
    await openExportForToday(page);
    await preview.click();
    await problems.getByRole('button', { name, exact: true }).click();
    await expect(page).toHaveURL(/\/beheer\/mensen\?open=/);
    const panel = page.locator('aside.st-panel');
    await expect(panel.getByRole('heading', { name })).toBeVisible();
    await panel.locator('#payroll-number').fill(`P-20${index}`);
    await panel.getByRole('button', { name: 'Personeelsnummer opslaan' }).click();
    await expect(panel.getByText('Opgeslagen')).toBeVisible();
  }

  // Nothing blocks any more: preview shows rows, the download is enabled.
  await openExportForToday(page);
  await preview.click();
  await expect(page.getByText(/^\d+ rij(en)?/).first()).toBeVisible();
  await expect(page.locator('table.st-table-preview tbody tr').first()).toBeVisible();
  await expect(download).toBeEnabled();
  await shot(page, '25-admin-export-preview');

  const [file] = await Promise.all([page.waitForEvent('download'), download.click()]);
  expect(file.suggestedFilename()).toMatch(/^[^/\\]+\.(csv|xlsx)$/);
  await expect(page.getByText(/Bestand gemaakt · controlegetal [0-9a-f]{12}$/)).toBeVisible();
  await shot(page, '26-admin-export-done');

  // The history lists the export; checking it again gives the same checksum.
  const history = page.locator('table').last();
  await expect(history.locator('tbody tr').first()).toContainText('Beheerder Test');
  await history.getByRole('button', { name: 'Controleren' }).first().click();
  await expect(history.getByText('Klopt').first()).toBeVisible();
});

test('7h. admin copies a profile, sees the errors of a wrong one and deactivates the copy', async () => {
  const { page } = admin;
  await page.goto('/beheer/instellingen');
  await page.getByRole('button', { name: 'Export', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Exportprofielen' })).toBeVisible();
  await page.getByRole('button', { name: /^Kopie maken: / }).first().click();
  const toml = page.getByLabel('Instellingen van het profiel');
  const original = await toml.inputValue();
  expect(original).toContain('profile_version');
  await page.getByLabel('Naam', { exact: true }).fill('Kopie voor test');

  await toml.fill(original + '\nonbekende_instelling = 1\n');
  await page.getByRole('button', { name: 'Bewaren' }).click();
  const errors = page.getByRole('alert').filter({ hasText: 'Het profiel klopt nog niet' });
  await expect(errors.locator('li').first()).toBeVisible();
  await shot(page, '27-admin-profile-errors');

  await toml.fill(original);
  await page.getByRole('button', { name: 'Bewaren' }).click();
  const copy = page.getByRole('listitem').filter({ hasText: 'Kopie voor test' });
  await expect(copy).toContainText('Eigen profiel');
  await copy.getByRole('button', { name: 'Uitschakelen' }).click();
  await copy.getByRole('button', { name: 'Ja', exact: true }).click();
  await expect(copy).toContainText('Uitgeschakeld');
});

test('7i. admin makes an API key, sees it once, uses it and revokes it', async ({ request }) => {
  const { page } = admin;
  await page.goto('/beheer/instellingen');
  await page.getByRole('button', { name: 'Integraties', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Integraties' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'API-documentatie' })).toHaveAttribute('href', '/api/v1/docs');
  await expect(page.getByRole('link', { name: 'API-documentatie' })).toHaveAttribute('target', '_blank');

  await page.getByRole('button', { name: 'Sleutel aanmaken' }).click();
  await page.getByRole('button', { name: 'Sleutel aanmaken' }).click();
  await expect(page.getByText('Geef een naam van 1 tot 100 tekens.')).toBeVisible();
  await page.getByLabel('Naam', { exact: true }).fill('Loonsync e2e');
  await page.getByRole('button', { name: 'Sleutel aanmaken' }).click();
  await expect(page.getByText('Kies minstens één recht.')).toBeVisible();
  await page.getByRole('checkbox', { name: /Mensen lezen/ }).check();
  await page.getByRole('button', { name: 'Sleutel aanmaken' }).click();

  const secret = page.locator('#new-api-key');
  await expect(secret).toBeVisible();
  const key = await secret.inputValue();
  expect(key).toMatch(/^tr_live_/);
  await expect(page.getByText('Bewaar deze sleutel nu. Je ziet hem niet meer.')).toBeVisible();
  await shot(page, '28-admin-integrations-key');

  const row = page.locator('li.st-key').filter({ hasText: 'Loonsync e2e' });
  await expect(row).toContainText('Actief');
  await expect(row).toContainText('Mensen lezen');
  await expect(row).toContainText('Alle scholen');
  await expect(row).not.toContainText(key);

  const auth = { Authorization: `Bearer ${key}` };
  expect((await request.get(origin() + '/api/v1/people', { headers: auth })).status()).toBe(200);

  // Once confirmed, the secret is gone from the page.
  await page.getByRole('button', { name: 'Ik heb de sleutel bewaard' }).click();
  await expect(page.locator('#new-api-key')).toHaveCount(0);

  await row.getByRole('button', { name: 'Intrekken' }).click();
  await row.getByRole('button', { name: 'Ja', exact: true }).click();
  await expect(row).toContainText('Ingetrokken');
  expect((await request.get(origin() + '/api/v1/people', { headers: auth })).status()).toBe(401);
  await shot(page, '29-admin-integrations-list');

  // The interactive documentation loads from this origin (the CSP check of scenario 8 covers it).
  await page.goto('/api/v1/docs');
  await expect(page.locator('.scalar-app').first()).toBeVisible();
  await expect(page.getByText('Tijdsregistratie API').first()).toBeVisible();
  await shot(page, '30-api-docs');
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

test('10. /setup hands over to the home page once setup is completed', async () => {
  const { page } = admin;
  await page.goto('/setup');
  await expect(page).toHaveURL(/\/(beheer|aanmelden)$/);
  await expect(page.getByRole('heading', { name: 'Welkom', exact: true })).toHaveCount(0);
});
