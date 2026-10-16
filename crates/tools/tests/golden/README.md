# Tools golden fixtures

Most of these fixtures record the output of the real TypeScript forecast
tools and their CLIs. The Rust tests compare `spot-price-tools` against them.
Do not edit them by hand.

The generator `scripts/golden/tools.ts` is in the Git history of the Rust
port PR at commit `8b4602d`. Check out that commit and run
`node --import tsx scripts/golden/tools.ts` outside the sandbox. The output
is deterministic.

## Rust-produced backtest sections

The backtest of the served forecast reads prices in the served window and
starts each series at the CET/CEST delivery-day boundary. The TypeScript
tools did not do this. So the `synthetic` and `dbCompare` sections of
`backtest_expected.json` come from the Rust tools. These sections are a
regression snapshot, not a parity check. The other sections still come from
the TypeScript tools.

To regenerate the two sections, run these commands from the repository root:

```sh
SQLX_OFFLINE=true cargo nextest run -p spot-price-tools --locked --run-ignored only -E 'test(regenerate_backtest_expected)'
pnpm exec prettier --ignore-path '' --write crates/tools/tests/golden/backtest_expected.json
```

The test keeps the other sections byte for byte. Review the diff before you
commit it. A golden change is a contract change.
