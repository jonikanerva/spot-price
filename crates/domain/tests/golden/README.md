# Domain golden fixtures

These fixtures record the output of the real TypeScript modules of the
former service. The Rust tests compare the domain crate against them. Do not
edit them by hand.

The generators are in the Git history of the Rust port PR:

| Directory   | Generator                       | Commit    |
| ----------- | ------------------------------- | --------- |
| `core/`     | `scripts/golden/domain-core.ts` | `afc25fc` |
| `forecast/` | `scripts/golden/forecast.ts`    | `09c680d` |

Check out the commit, run `scripts/test-db.sh start` if needed, and run
`node --import tsx <generator>` outside the sandbox. The output is
deterministic.

The served forecast trains with `LatestPublishedRule::Before`. The
TypeScript service used the at-or-before rule, so the forecast parity tests
pass `LatestPublishedRule::AtOrBefore` explicitly.

The served forecast takes its within-day shape from the four-week weekday
mean (`ForecastOptions::served`). The TypeScript service used the persistence
shape: the price 24 h earlier, else 168 h earlier. So the forecast parity
tests pass `ShapeProfile::Persistence` explicitly. The route test compares
the route with the served options and the TypeScript diagnostics with the
persistence shape.
