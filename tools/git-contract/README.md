# Git protocol oracle

A transport-free reference decoder for the agent's Git protocol, and its tests.
It is not imported by the application. An independent decoder catches wire
drift while native validators and RPC tests exercise the actual application.
This is an npm test tool, not a Rust crate.

From the repository root:

```sh
npm ci --prefix tools/git-contract
npm run check:contracts
npm run test:git-cli-contract
npm run test:git-contract # requires the disposable SSH/Docker fixtures
```

The first command installs only this tool's pinned dependencies. The live checks
require 27 successful method responses and 45 successful/conflict action kinds;
they also replay native page/diff/blob/operation validators. Additional response
fields are checked by the oracle. No single layer alone proves all GPUI behavior.
The `GitSession` interface here is an injected in-memory boundary, never a desktop
transport.
