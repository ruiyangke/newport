# Terminal dependency notices

These notices accompany Newport's statically linked terminal engine. Packaging
copies this directory into `Licenses/terminal` on every desktop platform.
They cover the new native terminal dependencies, not the application's entire
dependency graph. Font and GPUI Kit notices are packaged separately.

| Component | Source revision | Included notices |
| --- | --- | --- |
| Ghostty | `ghostty-org/ghostty` at `3425025e585a3403340d4dc6d65132f42e05e605` | `LICENSE-Ghostty` |
| Rust bindings | `Uzaaft/libghostty-rs` at `d2036baeb605d31d09a9e2ba6d0c923e04c34d54` | `LICENSE-libghostty-rs` |
| uucode | `jacobsandlund/uucode` at `9d55524551411b493cca41ca06363625d90aff1e` | `uucode/LICENSE.md` and its referenced `licenses/` files |
| Highway | `google/highway` at `66486a10623fa0d72fe91260f96c892e41aceb06` | `LICENSE-Highway-APACHE`, `LICENSE-Highway-BSD3` |
| simdutf | Ghostty's vendored 9.0.0 amalgamation; upstream tag `v9.0.0` resolves to `ca7acbcea967b5dcbab490066e99e3a6e6925539` | `LICENSE-simdutf-APACHE`, `LICENSE-simdutf-MIT`, `LICENSE-simdutf-TH` |

Ghostty, Rust bindings, uucode, and Highway license files are copied unchanged
from the pinned sources. The uucode supplementary licenses are copied from its
pinned Git source because its Zig package excludes that directory. The simdutf
MIT and Apache licenses are copied from the upstream revision above; the TH
notice is extracted verbatim from Ghostty's vendored `simdutf.h`.

Highway source files also carry `Copyright 2020 Google LLC` notices. Both
upstream license alternatives are retained here. Ghostty contains Newport
compatibility changes documented in `crates/newport-terminal/patches/README.md`;
the original copyright notices remain intact.

This list follows the VT module's build dependencies in `GhosttyZig.zig`:
uucode, generated Unicode tables, and SIMD dependencies. Kitty graphics is
disabled in Newport, so Wuffs is not linked into the VT module. Revisit these
notices when changing the source pins or enabling additional native features.
