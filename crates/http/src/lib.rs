//! Chummy's HTTP clients.
//!
//! - **The operating system checks certificates** ([`builder`]): Windows'
//!   certificate APIs, the macOS Keychain, or the system's certificate store
//!   on Linux. A snapshot of the system's root certificates isn't enough on
//!   Windows, which fetches roots it hasn't needed yet only when asked to
//!   verify a site; Chummy then failed to reach github.com on fresh Windows
//!   installs. The OS verifier also respects company and antivirus
//!   certificates the user installed, and revoked certificates. Nothing
//!   here ever turns verification off.
//! - **Errors say what went wrong** ([`describe`]): a certificate problem, no
//!   connection, a time-out or a proxy, rather than only "couldn't reach".

use std::error::Error as StdError;
use std::sync::Arc;

/// A client builder whose certificates are checked by the operating system.
/// (If the OS verifier can't be set up, reqwest's own checks against the
/// system's root certificates are used instead.)
pub fn builder() -> reqwest::ClientBuilder {
    // reqwest is built without a default TLS crypto provider: make sure the
    // process has one, so a client made without the OS verifier (when it
    // can't be set up) still works instead of panicking.
    static PROVIDER: std::sync::Once = std::sync::Once::new();
    PROVIDER.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
    let b = reqwest::Client::builder();
    match platform_tls() {
        Some(tls) => b.use_preconfigured_tls(tls),
        None => b,
    }
}

/// The TLS settings Chummy's web requests use (the operating system's
/// certificate checks), for connections that aren't made with reqwest
/// (a WebSocket). `None` when they can't be set up here.
pub fn tls_config() -> Option<rustls::ClientConfig> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    platform_tls()
}

fn platform_tls() -> Option<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = rustls_platform_verifier::Verifier::new(provider.clone()).ok()?;
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .ok()?
        // "dangerous" is rustls' name for using a verifier of one's own:
        // here, the operating system's. It doesn't skip verification.
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Some(config)
}

/// A short, plain description of why a request failed, for messages like
/// "couldn't reach github.com (…)".
pub fn describe(e: &reqwest::Error) -> String {
    let chain = chain_text(e);
    let lower = chain.to_lowercase();
    if lower.contains("certificate") || lower.contains("unknownissuer") || lower.contains("tls") {
        return format!(
            "its security certificate couldn't be checked; if your network or antivirus inspects \
             secure connections, your system may need its certificate ({})",
            last_line(&chain)
        );
    }
    if e.is_timeout() {
        return "it took too long to answer".into();
    }
    if lower.contains("tunnel") || lower.contains("proxy") {
        return format!("the proxy refused the connection ({})", last_line(&chain));
    }
    if lower.contains("dns") || lower.contains("resolve") || lower.contains("no such host") {
        return "its address couldn't be looked up; are you online?".into();
    }
    if e.is_connect() {
        return format!("no connection could be made ({})", last_line(&chain));
    }
    if let Some(s) = e.status() {
        return format!("it answered {}", s.as_u16());
    }
    last_line(&chain).to_string()
}

/// "github.com (why)", for "couldn't reach …" messages.
pub fn reached(host: String, e: &reqwest::Error) -> String {
    format!("{host} ({})", describe(e))
}

/// Every message in the error's chain of causes.
fn chain_text(e: &reqwest::Error) -> String {
    let mut parts = vec![e.to_string()];
    let mut source = e.source();
    while let Some(s) = source {
        let t = s.to_string();
        if !parts.iter().any(|p| p.contains(&t)) {
            parts.push(t);
        }
        source = s.source();
    }
    parts.join(": ")
}

/// The innermost cause, which is usually the informative one.
fn last_line(chain: &str) -> &str {
    chain.rsplit(": ").next().unwrap_or(chain).trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_os_verifier_is_set_up() {
        assert!(platform_tls().is_some());
        builder().build().unwrap();
    }

    #[tokio::test]
    async fn failures_are_described() {
        // Nothing listens on port 9 of this computer.
        let e = builder()
            .build()
            .unwrap()
            .get("http://127.0.0.1:9/")
            .send()
            .await
            .unwrap_err();
        let d = describe(&e);
        assert!(d.starts_with("no connection could be made"), "{d}");
    }
}

/// Needs the internet: `cargo test -p chummy-http -- --ignored`. Passes
/// when the certificate check succeeds (any HTTP answer will do).
#[cfg(test)]
mod online {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn certificates_are_checked_by_the_os() {
        let c = builder().user_agent("chummy-test").build().unwrap();
        for url in [
            "https://github.com/",
            "https://files.pythonhosted.org/",
            "https://pypi.org/simple/",
        ] {
            let r = c.head(url).send().await;
            assert!(r.is_ok(), "{url}: {}", describe(&r.unwrap_err()));
        }
    }
}
