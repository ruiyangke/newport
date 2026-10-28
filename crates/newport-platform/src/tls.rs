//! TLS for Newport's own HTTPS clients: updates and the account site.

/// Verifies servers against the operating system's trust store, as the
/// browsers on the same machine do. Authorities an administrator or MDM
/// installs, such as a TLS-inspecting proxy's, are trusted, and the system's
/// distrust decisions apply. Certificate and hostname checks stay on.
pub(crate) fn system_trust() -> ureq::tls::TlsConfig {
    let builder = ureq::tls::TlsConfig::builder();
    // Windows uses Schannel through native-tls; other systems use rustls.
    #[cfg(windows)]
    let builder = builder.provider(ureq::tls::TlsProvider::NativeTls);
    builder
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build()
}

#[cfg(test)]
mod tests {
    #[test]
    fn newport_trusts_what_the_system_trusts() {
        assert!(matches!(
            super::system_trust().root_certs(),
            ureq::tls::RootCerts::PlatformVerifier
        ));
    }
}
