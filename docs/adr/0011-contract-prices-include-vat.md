# 0011. The settings hold contract prices with VAT

- Status: accepted
- Date: 2026-11-18

## Context

A Finnish electricity invoice and a grid invoice show each unit price with
VAT. The settings took the margin, the transfer prices, and the tax without
VAT, and the price calculation added VAT to their sum. Only the margin
label of the setup page said "excl. VAT". The default tax, 2.82752 c/kWh,
was a price with VAT.

The owner's three terms versions held invoice numbers, so each price had
VAT twice. The fixed part of the price was about 1.46 c/kWh too high by day
and 0.92 c/kWh too high at night. The difference between day and night was
0.55 c/kWh too large, which can move the cheapest window.

The owner decided on 2026-11-18:

- The user enters and sees prices with VAT.
- The browser does no price calculation. The server does the conversion.
- The settings fields get names that say that the price has VAT.
- No `/api/v2`: the owner is the only caller of the settings routes.

## Decision

1. `contract_terms` holds the margin, the day and night transfer prices,
   and the tax with VAT, at the VAT percent of the same version. The
   columns keep their names.
2. The settings routes read and write these values unchanged. Their fields
   are `marginInclVatCentsKwh`, `transferDayInclVatCentsKwh`,
   `transferNightInclVatCentsKwh`, and `taxInclVatCentsKwh`. The old field
   names are unknown keys, which the routes ignore.
3. The price calculation does not change. It uses prices without VAT.
   `ContractTerms::settings` in the domain crate derives each one as
   `price × 100 / (100 + VAT)`. The price routes and their fields do not
   change: the parts of a price have no VAT, and `vatCentsKwh` is the VAT
   of the whole price.
4. The VAT percent of a version thus adds VAT to the spot price only. A
   request that changes only the VAT percent keeps the stored prices.
5. A stored version without a finite derived price, for example at a VAT
   of -100 %, gives the generic 500 and the log code `invalid_settings`.
6. No migration changes the stored rows. The owner confirmed that every
   stored row already holds prices with VAT, so the new reading corrects
   the price at the release.
7. The default tax of a new account is the tax of 2026 with VAT:
   2.8112 c/kWh of electricity tax plus 0.1067 c/kWh of security of supply
   fee. The source is a Helen Sähköverkko invoice for October 2026.

## Alternatives

- **Convert in the browser.** Rejected by the owner: price calculation
  belongs on the server, where unit tests cover it.
- **Store prices without VAT and convert in both directions on the
  server.** Rejected: a read must multiply a stored quotient again, so it
  needs a rounding rule. A request that changes only the VAT percent needs
  a second rule. The stored rows need a division migration.
- **Keep the old field names with the new meaning.** Rejected: the same
  name would mean a price with VAT in the settings and a price without VAT
  in the price routes.
- **Add VAT to the spot price only in the price calculation, and return
  the parts with VAT.** Rejected: it changes the meaning of the fields of
  every price route, and the recorded price bodies no longer apply.
- **Rename the columns.** Rejected: the running release reads the old
  column names during a deployment.

## Consequences

- The numbers of the setup page equal the numbers of the invoices.
- The column names and the field names of the tools option
  `--cost contract:` do not say whether a price has VAT. `STACK.md` §5
  records the meaning of the columns. The tools option still takes prices
  without VAT; it is a declared scenario, not a stored setting.
- The HTTP golden replay gives recorded settings requests the new field
  names. Its seed loader adds VAT to each seeded price and fails when the
  service does not derive the seeded price again bit for bit. The price
  bodies of the seeded users stay equal byte for byte.
- A caller that sends an old field name gets 200 and changes nothing.

## Cost of reversal

A return to prices without VAT needs a migration that divides each stored
price by `1 + VAT / 100`, the old field names, and the old labels. No data
is lost in either direction.

## Reassessment

Reassess when a user needs a contract whose invoice shows prices without
VAT, or when a second caller of the settings routes exists.
