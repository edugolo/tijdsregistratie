# Payroll export: research notes on Acerta Connect

Status: research (2026-11-24). The full Acerta import specification is not public; this is what
public sources confirm, plus what we infer. Use it to design export profiles, and confirm the exact
layout with Acerta (via the customer's payroll advisor) before relying on it.

## Confirmed

- Acerta Connect imports via configurable **import definitions** (Import > Nieuwe import). Data types:
  "kalenders" (calendar/time data) and "afwijkende loonelementen". Text files and Excel (one sheet per
  run) are supported; the file must match the chosen definition. Structure check, then content check;
  one wrong line for an employee rejects all of that employee's lines. Errors can be fixed in a grid
  before "Importeer". Source: https://help.booku.be/nl/articles/326509-technische-uitleg-over-acerta-connect
- Result grid fields: line number, overeenkomst (contract number), loonperiode, update code
  ("toevoegen, vervangen, …"), looncode, eenheden, bedrag per eenheid, percentage, bedrag, dagen,
  kostenplaats. Same source.
- Precondition: the contract's work schedule type must be "Vast (met tijdsregistratie)" or
  "Variabel (met tijdsregistratie)". Same source.
- A new import always overwrites the existing data for the period; correct in the source system and
  re-import. Source: https://help.booku.be/nl/articles/326547-hoe-geef-ik-mijn-tijdsregistraties-door-in-acerta-connect-van-a-tot-z
- The calendar data model (AFAS XML interface to Acerta Connect, block CALENDARPERIODS): reference
  date of the period, "renew" flag (replace period), calendar date, presence/absence code (looncode,
  max 6 chars), number of hours (decimal, 2 places). Per day, per looncode, decimal hours, keyed by
  the employee's contract / external employee number. Source: https://help.afas.nl/help/NL/SE/133599.htm
- The employee key is the Acerta **overeenkomstnummer** (contract number). Source:
  https://intercom.help/mbrella/nl/articles/5721170-importeer-je-payroll-exportbestand-in-acerta
- Acerta also offers REST APIs / file exchange for integrations (developer portal); no public docs
  for prestaties. Source: https://www.acerta.be/nl/softwarekoppelingen

## Not confirmed

Exact file layout (CSV vs fixed width), encoding, separator, header row, column order, date and
number formats, valid update codes, whether begin/end times are accepted (evidence suggests only
date + looncode + hours).

## Consequences for our design

Acerta itself works with configurable import definitions, which matches our approach: a neutral
export model plus declarative export profiles. The Acerta profile produces one row per employee per
day per looncode with decimal hours, keyed on the overeenkomstnummer, with a configurable mapping from
our presence/absence types to Acerta looncodes, and format options (CSV/XLSX, separator, decimal sign,
date format) so it can be aligned with the customer's import definition.

Reconstructed example (inferred, placeholder looncodes):

```
overeenkomst;datum;looncode;uren
1234567;2026-11-02;100;7,60
1234567;2026-11-03;100;7,60
```

## Ask Acerta

1. The technical specification for the calendar import ("Importeren - Importbestanden") and a sample
   import definition (text or Excel).
2. Whether a standard calendar import definition exists or one is configured per customer.
3. The looncodes for presence and absence used for this employer.
4. Access to the REST API for prestaties, if available.
