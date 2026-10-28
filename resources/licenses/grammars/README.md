# Grammar notices

These upstream notices accompany Newport's grammar libraries and their
Tree-sitter language binding. `SOURCES.json` records each crate version, archive
checksum, source revision, and the source and checksum of every notice. Notice
files are preserved byte for byte.

Notices included in published crates come from the checksum-verified crate
archive. Where a crate omits its notice, the notice comes from its upstream Git
revision recorded in the crate's `.cargo_vcs_info.json`.

`tree-sitter-proto` 0.2.0 declares MIT in its crate metadata but omits the license
file both from the archive and from its source revision. Its notice is retained
from upstream commit `d4aefd27a0f1f021ede6239b870a2e2a0119450d`, which added the
license file and included it in crate packaging. The inventory distinguishes
that notice source from the crate's source revision.

When updating a grammar dependency, update its inventory entry and preserve all
upstream notice files. Packaging verifies the inventory against the dependency
manifest, Cargo lockfile, and notice bytes before staging either desktop package.
The separate `LICENSE-GPUI-KIT` notice covers the adapted language configuration
and query material.
