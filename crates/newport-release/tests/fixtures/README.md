These are disposable test-only signing keys in the base64-wrapped,
password-encrypted Minisign format the release signer imports. They have never
signed a Newport distribution and are unrelated to production credentials.
`empty-password.key` uses an empty password; `fixture-password.key` uses
`fixture-password`. The Rust tests import both without generating keys at test
time.
