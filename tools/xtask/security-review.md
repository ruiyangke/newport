# RSA dependency review

Historical release review for maintainers. User guidance is in [Security and privacy](../../docs/security.md).

Release disposition (15 October 2026): the maintainer explicitly accepted the
RustCrypto RSA issue for this release. Newport retains its existing SSH signer
and RSA key compatibility. This is risk acceptance, not a vulnerability fix or
evidence that the application is unaffected. The advisory remains documented
and visible in dependency audit results; no audit suppression was added.

As of 14 October 2026, the SSH dependencies include an RSA implementation affected by [RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html), a timing side-channel vulnerability with no reported patch. RSA support remains enabled for compatibility. The call-path review below narrows the finding but does not establish that Newport is unaffected.

### RSA call-path review

- The lockfile contains RSA 0.9.10 through SQLx's optional MySQL dependency.
  `cargo tree --locked --target <target> --features
  newport-platform/updates,newport-release/signing -i rsa@0.9.10` has no active
  dependency path for macOS ARM64, Windows ARM64 or Windows x64. Newport enables
  SQLx's SQLite backend, not MySQL. A lockfile match alone is not evidence that
  this version is linked into those applications.
- RSA 0.10.0-rc.18 is active through `russh` 0.63.3 and `ssh-key` 0.7.0-rc.11
  on all three targets. Newport's SSH authentication can sign with RSA private
  keys. `russh::helpers::sign_with_hash_alg` uses `signature::Signer`, whose
  `ssh-key` RSA implementation calls the PKCS#1 v1.5 signing key's `try_sign`.
  The RSA crate's non-randomized signing implementation passes no blinding RNG.
- No application or SSH-transport call to RSA private-key decryption was found
  in the reviewed sources. That is a bounded source observation, **not** proof
  that timing attacks against private-key operations are impossible. The
  remaining signing path is covered by the explicit release risk acceptance above.

Upstream's [constant-time padding work](https://github.com/RustCrypto/RSA/issues/626)
remains relevant; upgrading to the current release candidate alone does not
resolve the advisory. Do not suppress it or remove existing users' RSA keys as
a release-preparation shortcut. Recheck the exact-target dependency graph when updating dependencies; historical
review is not evidence for a different build.

Some dependencies also have maintenance warnings. Dependency checks are not a comprehensive security audit.
