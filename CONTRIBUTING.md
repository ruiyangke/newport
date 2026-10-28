# Contributing to Newport

How to write and change code in this repository. Learn what exists from the
repository itself; this guide covers how to work in it.

## Keep upgrades working

Newport ships to users, and its agent runs on their servers. Every change must
keep an existing install working:

- A release opens the profile and stored credentials an earlier release wrote.
  Preserve the profile layout, the credential and profile identities, and the
  update signing key across releases.
- A new desktop meets older agents, and a new agent meets older desktops. Add a
  protocol version instead of changing an existing one.
- Inside the code there is no compatibility layer to keep. Rename a symbol,
  delete the old name and update every caller in the same change.

## Working in the repository

- Run every build, check, test and package step through `cargo xtask`.
- A change that needs newport-remote lands there first, following its own
  [CONTRIBUTING.md](https://github.com/ruiyangke/newport-remote/blob/main/CONTRIBUTING.md).
  Then move `.github/newport-remote-revision` to that commit.
- Never open a real profile from a development build. Use the disposable
  `target/dev-profile`, or set `NEWPORT_DATA_DIR` to another isolated one.
- Never act on an installed Newport. Development builds can share its bundle
  ID; stop a development instance by its process ID.
- Keep signing identities, provisioning profiles and other local settings out
  of git.
- Run the whole suite of the crate you touched, not just `--lib`, and
  `cargo xtask check` before you push.
- A passing headless suite does not prove GUI behaviour, Keychain access or a
  signed upgrade. Check those in the running app and record what you checked.

## Writing code

- **Follow the platform's standards first.** freedesktop and XDG
  specifications and portals on Linux, Apple's conventions on macOS. Invent a
  mechanism only where none exists.
- **Server work goes through the agent.** Features that read or change a
  server use the agent's workers, not shell scripts run over SSH.
- **The terminal stays a terminal.** Graphical apps started from a terminal
  open in their own windows; their failures never block the terminal.
- **Private crates stay out of Cargo files.** No `Cargo.toml`, `Cargo.lock` or
  Cargo configuration names a `newport-remote` crate; only the `--remote-apps`
  commands add them for a build.
- **Every fix adds a regression test** that fails before the fix.
- **Prefer a library to hand-rolled code**, and reuse the shared controls,
  theme and helpers before adding new ones.
- **User-facing changes update the docs** under `docs/`, following
  [docs/AGENTS.md](docs/AGENTS.md).
- **No tombstones.** Leave no comments, docs or tests about what was removed or
  replaced; git history keeps it.

## Comments

- **Clear, direct and concise.** Plain words, short sentences, one point per
  comment.
- **Not verbose.** If a comment needs a paragraph, the code or its names need
  work first.
- **Say why, not what.** Don't repeat the logic the code already states. Explain
  the constraint, the reason or the non-obvious consequence.
- **No numbers that go stale.** No measurements, counts, sizes or values copied
  from the code. Name the constant instead of restating it.
- **No tombstones.** Don't describe what the code used to do, what was removed
  or what replaced it.
- **No process markers.** See below.

```rust
// Retry three times, 500 ms apart (was two before the handshake fix).   <- restates the code, stale numbers, tombstone
// C9: shared frames, see P3 step B.                                     <- process markers
// A timed-out handshake opened no channel, so retrying it is safe.      <- why
```

## No process markers

Plan, task and issue identifiers, phase numbers and step letters never appear
in code or comments: not in test, function, type or module names, feature
flags, log messages, doc comments or inline comments. Code outlives the plan,
and a marker means nothing to a reader who wasn't there. Keep them in plans,
trackers and pull requests.

Name and describe the behaviour:

```
g4_popup_fix                                   <- what plan item was this?
a_dismissed_popup_drops_its_pending_capture    <- what breaks if it fails
```

## Commit messages

Keep `git log` a readable, greppable changelog. Use Conventional Commits:

```
type(scope): imperative summary of what the change does
```

- **Type:** `fix`, `feat`, `refactor`, `perf`, `test`, `docs`, `build`, `ci`,
  `release`, `chore` or `revert`. Pick exactly one.
- **Scope:** one lowercase word for the area: a crate without its `newport-`
  prefix, `agent`, `xtask`, or a feature. Reuse a scope from `git log` before
  inventing one; leave it out when a change spans the repository.
- **Subject:** imperative and present tense, lowercase after the colon, no
  trailing period. Describe the effect ("keep copied text after the app
  closes"), not the mechanics ("update clipboard code"). Aim for 72 characters.
- **No process markers:** no plan or task IDs, phase numbers or step letters.
- **Body:** usually omitted. Add one when the reason is not obvious. Logs and
  narratives belong in the pull request or under `docs/`.
- **Attribution:** no `Co-authored-by` or other attribution lines.
- **Length:** the whole message stays within 150 characters.

Commit only the paths you changed (`git commit -m "…" -- <paths>`); others may
have work staged in the same checkout.

Enable the hooks once per clone with `git config core.hooksPath .githooks`.
They check the message length, attribution lines and private crate names.
Never bypass them with `--no-verify`.

### Checklist

- [ ] `type(scope): ...`, imperative, lowercase after the colon, no period
- [ ] Describes a real outcome; 150 characters or fewer
- [ ] A regression test accompanies every bug fix
- [ ] `cargo xtask check` passes
