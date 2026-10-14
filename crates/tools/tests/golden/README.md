# Tools golden fixtures

These fixtures record the output of the real TypeScript forecast tools and
their CLIs. The Rust tests compare `spot-price-tools` against them. Do not
edit them by hand.

The generator `scripts/golden/tools.ts` is in the Git history of the Rust
port PR at commit `8b4602d`. Check out that commit and run
`node --import tsx scripts/golden/tools.ts` outside the sandbox. The output
is deterministic.
