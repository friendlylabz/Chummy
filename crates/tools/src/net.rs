//! Web requests made on a tool's behalf.
//!
//! A tool only reaches the internet through `chummy.http`, which asks the
//! app. The app makes the request only when the tool's internet access is
//! on, only to `http`/`https` addresses, and never to this computer or the
//! local network (loopback, private, link-local and similar addresses), so a
//! tool can't reach a router, a printer or a local service. Redirects are
//! followed by hand so every hop is checked. Responses are size-limited.

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{Value as Json, json};

const MAX_BODY: usize = 2 * 1024 * 1024;
const MAX_REQUEST_BODY: usize = 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
const TIMEOUT: Duration = Duration::from_secs(20);

/// Whether a call may use the internet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetPolicy {
    pub allowed: bool,
    /// Why not, in words for the model, when `allowed` is false.
    pub refusal: String,
    /// Tests only: allow local addresses.
    pub allow_local: bool,
}

impl NetPolicy {
    pub fn off(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            refusal: reason.into(),
            allow_local: false,
        }
    }
    pub fn on() -> Self {
        Self {
            allowed: true,
            ..Default::default()
        }
    }
}

/// Addresses on this computer or the local network.
pub fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_local_v4(v4),
        IpAddr::V6(v6) => {
            // IPv4 inside IPv6: mapped (::ffff:a.b.c.d), the old
            // compatible form (::a.b.c.d) and 6to4 (2002:aabb:ccdd::).
            if let Some(v4) = v6.to_ipv4() {
                return is_local_v4(v4);
            }
            let s = v6.segments();
            if s[0] == 0x2002 {
                let [a, b] = s[1].to_be_bytes();
                let [c, d] = s[2].to_be_bytes();
                if is_local_v4(Ipv4Addr::new(a, b, c, d)) {
                    return true;
                }
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local
                || (s[0] & 0xffc0) == 0xfe80 // link-local
                || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
                || (s[0] == 0x0064 && s[1] == 0xff9b) // NAT64
        }
    }
}

fn is_local_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || o[0] == 0
        || (o[0] == 100 && (64..128).contains(&o[1])) // carrier-grade NAT
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19)) // benchmarking
        || o[0] >= 240
}

fn proxy_configured() -> bool {
    [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ]
    .iter()
    .any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()))
}

/// Checks a URL's scheme and where its host points. Returns the addresses
/// it checked, so the connection can be pinned to them (otherwise a second
/// lookup could answer differently: DNS rebinding).
async fn check_url(
    url: &reqwest::Url,
    policy: &NetPolicy,
) -> Result<Vec<std::net::SocketAddr>, String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "only http and https addresses work, not {}:",
            url.scheme()
        ));
    }
    let host = url.host_str().ok_or("the address has no host")?;
    if policy.allow_local {
        return Ok(vec![]);
    }
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return if is_local(ip) {
            Err("addresses on this computer or the local network can't be reached".into())
        } else {
            Ok(vec![])
        };
    }
    if bare.eq_ignore_ascii_case("localhost")
        || bare.ends_with(".localhost")
        || bare.ends_with(".local")
    {
        return Err("addresses on this computer or the local network can't be reached".into());
    }
    let port = url.port_or_known_default().unwrap_or(443);
    match tokio::net::lookup_host((bare, port)).await {
        Ok(addrs) => {
            let addrs: Vec<_> = addrs.collect();
            if addrs.is_empty() {
                return Err(format!("couldn't find {host}"));
            }
            if addrs.iter().any(|a| is_local(a.ip())) {
                return Err(format!(
                    "{host} points to this computer or the local network, which can't be reached"
                ));
            }
            Ok(addrs)
        }
        // Behind a proxy the computer may not resolve names itself; the
        // proxy does, and it's outside the local network.
        Err(_) if proxy_configured() => Ok(vec![]),
        Err(_) => Err(format!("couldn't find {host}")),
    }
}

/// Performs a request from a tool. `msg` is the tool's request:
/// `{ method, url, headers, body }`.
pub async fn http_request(
    client: &reqwest::Client,
    policy: &NetPolicy,
    msg: &Json,
) -> Result<Json, String> {
    if !policy.allowed {
        return Err(if policy.refusal.is_empty() {
            "This tool doesn't have internet access.".into()
        } else {
            policy.refusal.clone()
        });
    }
    let method = match msg.get("method").and_then(|m| m.as_str()).unwrap_or("GET") {
        "GET" => reqwest::Method::GET,
        "POST" => reqwest::Method::POST,
        other => return Err(format!("{other} requests aren't supported")),
    };
    let raw = msg.get("url").and_then(|u| u.as_str()).unwrap_or_default();
    let mut url = reqwest::Url::parse(raw).map_err(|_| format!("“{raw}” isn't a web address"))?;
    let body = msg
        .get("body")
        .and_then(|b| b.as_str())
        .unwrap_or_default()
        .to_string();
    if body.len() > MAX_REQUEST_BODY {
        return Err("the request body is too large".into());
    }
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(h) = msg.get("headers").and_then(|h| h.as_object()) {
        for (k, v) in h {
            let lower = k.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "host" | "content-length" | "connection" | "transfer-encoding"
            ) {
                continue;
            }
            if let (Ok(name), Some(Ok(val))) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                v.as_str().map(reqwest::header::HeaderValue::from_str),
            ) {
                headers.insert(name, val);
            }
        }
    }

    let mut method = method;
    let mut body = Some(body);
    for _ in 0..=MAX_REDIRECTS {
        let addrs = check_url(&url, policy).await?;
        // Connect only to the addresses that were checked (otherwise a
        // second lookup could answer differently). Through a proxy this has
        // no effect, but hosts the proxy skips (NO_PROXY) connect directly.
        let pinned;
        let client = if !addrs.is_empty() {
            pinned = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(15))
                .user_agent(concat!("Chummy/", env!("CARGO_PKG_VERSION"), " (tool)"))
                .resolve_to_addrs(url.host_str().unwrap_or_default(), &addrs)
                .build()
                .map_err(|e| e.to_string())?;
            &pinned
        } else {
            client
        };
        let mut req = client
            .request(method.clone(), url.clone())
            .headers(headers.clone())
            .timeout(TIMEOUT);
        if method == reqwest::Method::POST
            && let Some(b) = &body
        {
            req = req.body(b.clone());
        }
        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                format!(
                    "{} didn't answer in time",
                    url.host_str().unwrap_or("the server")
                )
            } else {
                format!("couldn't reach {}", url.host_str().unwrap_or("the server"))
            }
        })?;
        let status = resp.status();
        if status.is_redirection()
            && let Some(loc) = resp.headers().get(reqwest::header::LOCATION)
        {
            let loc = loc.to_str().map_err(|_| "bad redirect")?;
            url = url.join(loc).map_err(|_| "bad redirect")?;
            if status != reqwest::StatusCode::TEMPORARY_REDIRECT
                && status != reqwest::StatusCode::PERMANENT_REDIRECT
            {
                method = reqwest::Method::GET;
                body = None;
            }
            continue;
        }
        let mut out_headers = serde_json::Map::new();
        for (k, v) in resp.headers() {
            if let Ok(s) = v.to_str() {
                out_headers
                    .entry(k.as_str().to_ascii_lowercase())
                    .or_insert_with(|| json!(s));
            }
        }
        let final_url = resp.url().to_string();
        let mut data = Vec::new();
        let mut truncated = false;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| "the download was cut off".to_string())?;
            data.extend_from_slice(&chunk);
            if data.len() > MAX_BODY {
                data.truncate(MAX_BODY);
                truncated = true;
                break;
            }
        }
        return Ok(json!({
            "status": status.as_u16(),
            "url": final_url,
            "headers": out_headers,
            "text": String::from_utf8_lossy(&data),
            "truncated": truncated,
        }));
    }
    Err("too many redirects".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_addresses() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.0.1",
            "172.16.5.5",
            "169.254.1.1",
            "0.0.0.0",
            "100.64.0.1",
            "::1",
            "fe80::1",
            "fd00::1",
            "::ffff:192.168.1.1",
            "::127.0.0.1",
            "2002:7f00:1::1",
            "2002:c0a8:101::1",
            "64:ff9b::7f00:1",
        ] {
            assert!(is_local(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "93.184.216.34",
            "1.1.1.1",
            "2606:4700::1111",
            "2002:5db8:d822::1",
        ] {
            assert!(!is_local(ip.parse().unwrap()), "{ip}");
        }
    }

    #[tokio::test]
    async fn refuses_when_off_or_local() {
        let c = reqwest::Client::new();
        let off = NetPolicy::off("Internet access is off for this tool.");
        let e = http_request(&c, &off, &json!({ "url": "https://example.com" })).await;
        assert_eq!(e.unwrap_err(), "Internet access is off for this tool.");

        let on = NetPolicy::on();
        for url in [
            "http://127.0.0.1:8080/",
            "http://localhost/",
            "http://[::1]/",
            "file:///etc/passwd",
            "http://192.168.1.1/admin",
        ] {
            assert!(
                http_request(&c, &on, &json!({ "url": url })).await.is_err(),
                "{url}"
            );
        }
    }
}
