# Security

- **Saved credentials:** profiles and SSH passwords are encrypted, with the unlock key stored in macOS Keychain. Metric history is not encrypted. See [storage and recovery](storage.md).
- **Server trust:** SSH host keys are trusted on first use. Changed or revoked keys are rejected.
- **Clipboard sharing:** enable it only for trusted servers. Sensitive content you copy is shared too, and remote files may remain after a lost connection.
- **Remote access:** actions use your SSH account's permissions. Port discovery may use existing non-interactive sudo access. Forwarded ports are accessible only through local loopback.

## Known limitation

As of 21 September 2026, the SSH dependencies include an RSA implementation affected by [RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html), a timing side-channel vulnerability with no reported patch. RSA support remains enabled for compatibility. Whether Porthop exposes the attack path has not been assessed.

Some dependencies also have maintenance warnings. Dependency checks are not a comprehensive security audit.
