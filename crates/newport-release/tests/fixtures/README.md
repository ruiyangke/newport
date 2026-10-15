These are disposable test-only signing keys, generated with Tauri CLI 2.11.5.
They have never signed a Newport distribution and are unrelated to production
credentials. `tauri-empty.key` uses an empty password; `tauri-password.key` uses
`fixture-password`. The Rust tests preserve compatibility with both formats
without requiring Tauri to generate keys at test time.
