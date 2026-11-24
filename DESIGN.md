# DESIGN.md

Design guide for the time registration app. Mockups: `docs/design/flows-and-screens.{html,pdf,png}`.
The organisation name and logo in the mockups are placeholders; branding is configurable per deployment.

## 1. Who we design for

- **Employees**: cleaning, technical, kitchen and supervision staff, often with limited digital skills,
  not always Dutch as home language, usually only a smartphone (sometimes old). They use the app twice
  a day for a few seconds.
- **Managers**: school principals or site coordinators. Busy, on a laptop or phone, want an overview at
  a glance and no extra admin.
- **Admins**: HR or IT for the whole organisation. Occasional use.

## 2. Principles

1. **One thing per screen.** The employee home screen has one action: START or STOP.
2. **Under 10 seconds, at most 2–3 taps** from opening the app to a confirmed registration.
3. **Always confirm with a time.** "Gestart om 07:42", never just "OK".
4. **Never block the employee for a technical reason.** Out of zone, no location, no network: store,
   flag, explain in one sentence.
5. **No technical words.** No "passkey", "GPS", "sessie", "token", "server" in employee-facing copy.
6. **Respect.** Explain why location is used before asking. Never imply distrust.
7. **Managers get status, not data.** Lists with clear states; details only on demand.

## 3. Copy

- Language: Flemish (nl-BE) first; fr-BE and en later. All strings via the i18n module.
- Register: informal **"je"**.
- Short sentences, everyday words, max ~12 words per sentence on employee screens.
- Sentence case for everything (titles, buttons). Exception: the main action buttons **START** and
  **STOP** in capitals.
- Quotes: “…”. Never low „…” quotes.
- No literal arrow characters in UI text; use icons.
- Times as `07:42`, durations as `7 u 18`, dates as `di 19 nov`.

### Glossary

| Concept | Use | Avoid |
|---|---|---|
| passkey | sleutel | passkey, wachtwoord |
| enroll a device | toestel koppelen | registreren, enrollen |
| linked | gekoppeld | geregistreerd |
| log in | aanmelden | inloggen, sign in |
| start / stop working | starten / stoppen | in- / uitklokken |
| site | school (or the configured site label) | vestigingsplaats, locatie-ID |
| manager | verantwoordelijke | manager, admin |
| HR role | personeelsdienst | HR, admin |
| admin role | beheerder | admin |
| out of zone | niet op school | buiten geofence |

### Standard messages

| Situation | Message |
|---|---|
| Started | Gestart om 07:42 |
| Stopped | Gestopt om 15:30 |
| Out of zone | Je bent niet op school. We hebben je start toch bewaard. Je verantwoordelijke ziet dit. |
| Location denied | We weten niet waar je bent. We hebben je start toch bewaard. |
| Offline | Bewaard. We sturen het door zodra je internet hebt. |
| Enrollment code expired | Deze code werkt niet meer. Vraag een nieuwe aan je verantwoordelijke. |
| New phone | Nieuwe gsm? Vraag het aan je verantwoordelijke. |

## 4. Visual language

Colours are Material 3 roles, generated from one seed: the organisation's brand colour
(default `#1e3a8a`, configurable in the settings). Nothing in the CSS hard-codes a colour.

**Seed to scheme.** The server turns the seed into a full scheme with the Material `SchemeFidelity`
variant at standard contrast and serves it as `/theme.css` (crate `crates/theme`). Fidelity keeps the
seed recognisable as the primary container; the other roles are derived tones of the same hue. The
sheet defines custom properties `--md-<role>` for light on `:root`, and the dark set both under
`@media (prefers-color-scheme: dark)` and under `:root[data-theme="dark"]`. `data-theme="light"`
forces light. The reference values for the default seed are in `docs/design/m3-scheme.json`.

| Role (`--md-…`) | Use |
|---|---|
| `primary`, `on-primary` | primary buttons, header logo, focus ring |
| `primary-container`, `on-primary-container` | brand accents |
| `secondary-container`, `on-secondary-container` | selected states, icon tiles |
| `tertiary-container`, `on-tertiary-container` | complementary accent |
| `error`, `on-error`, `error-container`, `on-error-container` | failures, destructive actions |
| `surface`, `surface-container-lowest` … `-highest` | app background and card levels |
| `on-surface`, `on-surface-variant` | main and secondary text |
| `outline`, `outline-variant` | borders, dividers |
| `inverse-surface`, `inverse-on-surface` | snackbars, preview frames |
| `ok`, `ok-container`, `on-ok-container` | in zone, "gekoppeld" |
| `warn`, `warn-container`, `on-warn-container` | out of zone, no location, no device |
| `start`, `on-start` / `stop`, `on-stop` | the START and STOP circle |

Status roles (ok, warn, start, stop) start from fixed greens, ambers and reds and are harmonised
towards the seed hue, with the same tone rules as the other roles (container 90/30, on-container
10/90, solid 40/70). Text always uses the matching `on-*` role of its background.

**Light, dark, system.** The default follows the system. Each device can pick Systeem, Licht or
Donker (component `AppearancePicker`); the choice is kept in `localStorage` (`tr-theme`) and applied
as `data-theme` on `<html>` by the app on start, so a device that overrides its system setting can
flash briefly on load.

- Status is never conveyed by colour alone: always an icon and a word as well.
- Status colours keep at least 4.5:1 contrast on their container in light and dark.
- Typography: system font stack. Body 16 px minimum on employee screens; titles 20–24 px; the main
  action label 24 px bold.
- Icons: Lucide, 2 px stroke.
- Radius: 12 px for buttons and cards, full circle for the main action button.

## 5. Components

- **Main action button**: circle, 150 px minimum diameter, green START / red STOP, label in capitals,
  soft shadow. Disabled state with spinner while the location is read (max 10 s).
- **Primary button**: full width on mobile, 48 px minimum height, brand colour.
- **Secondary button**: outlined, same size.
- **Status chip**: small pill with icon + word (Gekoppeld, Geen toestel, Gestart, Niet op school).
- **Notice card**: icon + one or two sentences; used for messages and warnings.
- **Day summary card**: rows Gestart / Gestopt / Pauze / Gewerkt.
- **QR panel**: QR code at least 200 px, name of the person next to it, countdown, cancel.

## 6. Screens

Employee (mobile first):

| Id | Screen | Notes |
|---|---|---|
| E1 | Welkom / installeer de app | opened from the QR code; Android install button |
| E1b | Zet de app op je iPhone | 3 illustrated steps; shown in Safari when not installed |
| E2 | Maak je sleutel aan | passkey registration |
| E3 | Waarom je locatie? | shown once before the browser permission prompt |
| E4 | Startscherm | greeting, site chip, START, optional message card |
| E5 | Bezig | "Gestart om …", elapsed time, STOP |
| E6 | Niet op school | warning after an out-of-zone registration; never blocks |
| E7 | Gestopt | day summary |
| E8 | Aanmelden | only when the session expired; one button |

Manager (laptop and mobile):

| Id | Screen | Notes |
|---|---|---|
| M1 | Mijn team | per site; status today; "Medewerker toevoegen"; "Koppel toestel" |
| M2 | Toestel koppelen | QR panel with countdown; switches to "Gekoppeld" automatically |

Admin:

| Id | Screen | Notes |
|---|---|---|
| A1 | Scholen | name, address, map pin + radius, break length, active |
| A2 | Personen en rollen | assign manager to sites, make admin, deactivate |
| A0 | Organisatie | name, short name, colour, logo, time zone |

First-run setup (only when there is no admin yet; laptop first, works on phone):

| Id | Screen | Notes |
|---|---|---|
| S1 | Welkom | asks the setup code from the server log; explains what follows (5 short steps) |
| S2 | Je organisatie | name, short name, colour swatches, optional logo, time zone |
| S3 | Je eerste school | name, address, "Gebruik mijn huidige locatie" or coordinates, radius, break |
| S4 | Over jou | your name; checkbox "Ik ben ook verantwoordelijke van deze school" |
| S5 | Maak je sleutel aan | passkey on this device |
| S6 | Klaar | next steps: verantwoordelijken toevoegen, medewerkers toevoegen, toestellen koppelen |

Wizard rules: progress indicator ("Stap 2 van 5"), one topic per screen, "Vorige" always available,
sensible defaults pre-filled, nothing saved until S5 succeeds (then everything in one transaction).

## 7. Interaction details

- Tap START/STOP: button shows a spinner, location is read, then the confirmation appears in the same
  screen. No navigation, no extra confirm dialog.
- Double tap protection: the button is disabled until the server answers or the action is queued.
- Haptic feedback on success where supported.
- Errors are shown in place, in one sentence, with one action ("Probeer opnieuw").

## 8. Accessibility

- WCAG 2.2 AA.
- Tap targets at least 48 × 48 px.
- Works at 200 % text size without horizontal scrolling.
- Screen reader labels on all icon-only elements; the main button announces its state ("Start je dag").
- Respects `prefers-reduced-motion`.
- Works on small (320 px wide) and old devices; no heavy animations or large images.

## 9. PWA

- Standalone display, brand colour as theme colour, maskable icon.
- Install guidance is part of enrollment (E1/E1b), not a separate help page.
- Opening the installed app goes straight to E4/E5 when the session is valid.
