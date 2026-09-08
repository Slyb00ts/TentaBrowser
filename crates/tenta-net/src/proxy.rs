//! HTTP CONNECT and SOCKS5 proxies (`docs/spec/03-network.md` §1).
//!
//! Proxy credentials never appear in logs or in error messages
//! (`docs/spec/19-security.md` §7): `Debug` and `Display` redact them.

use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

use crate::error::{ErrorName, NetError};

/// Which protocol the proxy speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProxyKind {
    /// HTTP CONNECT tunnel.
    Http,
    /// SOCKS5. `remote_dns` is the `socks5h` behaviour: the name is sent to
    /// the proxy instead of being resolved locally.
    Socks5 { remote_dns: bool },
}

/// Proxy credentials. Never printed.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ProxyAuth {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for ProxyAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProxyAuth(<redacted>)")
    }
}

/// One proxy, per client or per request.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Proxy {
    pub kind: ProxyKind,
    pub host: String,
    pub port: u16,
    pub auth: Option<ProxyAuth>,
}

impl std::fmt::Debug for Proxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Proxy")
            .field("kind", &self.kind)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("auth", &self.auth.is_some())
            .finish()
    }
}

impl Proxy {
    /// Parses `http://user:pass@host:port`, `socks5://host:port` or
    /// `socks5h://host:port` (the `h` form resolves names at the proxy).
    pub fn parse(url: &str) -> Result<Self, NetError> {
        let parsed = Url::parse(url).map_err(|err| {
            NetError::new(
                ErrorName::InvalidArgument,
                format!("invalid proxy URL: {err}"),
            )
        })?;
        let kind = match parsed.scheme() {
            "http" => ProxyKind::Http,
            "socks5" => ProxyKind::Socks5 { remote_dns: false },
            "socks5h" => ProxyKind::Socks5 { remote_dns: true },
            other => {
                return Err(NetError::new(
                    ErrorName::InvalidArgument,
                    format!("proxy scheme {other:?} is not supported"),
                ));
            }
        };
        let host = parsed
            .host_str()
            .ok_or_else(|| NetError::new(ErrorName::InvalidArgument, "proxy URL has no host"))?
            .to_owned();
        let port = parsed.port().unwrap_or(match kind {
            ProxyKind::Http => 8080,
            ProxyKind::Socks5 { .. } => 1080,
        });
        let auth = if parsed.username().is_empty() {
            None
        } else {
            Some(ProxyAuth {
                username: percent_decode(parsed.username()),
                password: percent_decode(parsed.password().unwrap_or("")),
            })
        };
        Ok(Self {
            kind,
            host,
            port,
            auth,
        })
    }

    /// The address to open a TCP connection to.
    pub fn address(&self) -> (String, u16) {
        (self.host.clone(), self.port)
    }

    /// Identity of the proxy for the pool key; never includes credentials.
    pub fn pool_key(&self) -> String {
        let scheme = match self.kind {
            ProxyKind::Http => "http",
            ProxyKind::Socks5 { remote_dns: false } => "socks5",
            ProxyKind::Socks5 { remote_dns: true } => "socks5h",
        };
        format!("{scheme}://{}:{}", self.host, self.port)
    }

    /// Whether the proxy resolves the destination name itself.
    pub fn resolves_remotely(&self) -> bool {
        matches!(self.kind, ProxyKind::Socks5 { remote_dns: true })
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(value) = u8::from_str_radix(hex, 16)
        {
            out.push(value);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Opens a CONNECT tunnel to `host:port` over an established proxy stream.
/// `user_agent` is sent like Chromium's `HttpProxyConnectJob`.
pub async fn http_connect<S>(
    stream: &mut S,
    host: &str,
    port: u16,
    auth: Option<&ProxyAuth>,
    user_agent: Option<&str>,
) -> Result<(), NetError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let authority = format_authority(host, port);
    let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some(auth) = auth {
        let token = base64::engine::general_purpose::STANDARD
            .encode(format!("{}:{}", auth.username, auth.password));
        request.push_str(&format!("Proxy-Authorization: Basic {token}\r\n"));
    }
    if let Some(ua) = user_agent {
        request.push_str(&format!("User-Agent: {ua}\r\n"));
    }
    request.push_str("Proxy-Connection: keep-alive\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|err| tunnel_error(&err))?;
    stream.flush().await.map_err(|err| tunnel_error(&err))?;

    let head = read_head(stream).await?;
    let status_line = head.lines().next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| {
            NetError::new(
                ErrorName::TunnelConnectionFailed,
                "proxy sent no status line",
            )
        })?;
    match status {
        200..=299 => Ok(()),
        407 => Err(NetError::new(
            ErrorName::ProxyAuthRequested,
            "proxy requires authentication",
        )),
        other => Err(NetError::new(
            ErrorName::TunnelConnectionFailed,
            format!("proxy refused CONNECT with status {other}"),
        )),
    }
}

/// Reads an HTTP head (up to the blank line) with a hard size limit.
async fn read_head<S>(stream: &mut S) -> Result<String, NetError>
where
    S: tokio::io::AsyncRead + Unpin,
{
    let mut buffer = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        let read = stream
            .read(&mut byte)
            .await
            .map_err(|err| tunnel_error(&err))?;
        if read == 0 {
            return Err(NetError::new(
                ErrorName::TunnelConnectionFailed,
                "proxy closed the connection during CONNECT",
            ));
        }
        buffer.push(byte[0]);
        if buffer.ends_with(b"\r\n\r\n") {
            break;
        }
        if buffer.len() > 8192 {
            return Err(NetError::new(
                ErrorName::TunnelConnectionFailed,
                "proxy response head is too large",
            ));
        }
    }
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

/// Performs the SOCKS5 handshake and CONNECT command (RFC 1928, RFC 1929).
pub async fn socks5_connect<S>(
    stream: &mut S,
    host: &str,
    port: u16,
    auth: Option<&ProxyAuth>,
) -> Result<(), NetError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    // Greeting: no authentication, and username/password when we have some.
    let methods: &[u8] = if auth.is_some() {
        &[0x00, 0x02]
    } else {
        &[0x00]
    };
    let mut greeting = vec![0x05, methods.len() as u8];
    greeting.extend_from_slice(methods);
    stream
        .write_all(&greeting)
        .await
        .map_err(|err| socks_error(&err))?;

    let mut choice = [0u8; 2];
    stream
        .read_exact(&mut choice)
        .await
        .map_err(|err| socks_error(&err))?;
    if choice[0] != 0x05 {
        return Err(NetError::new(
            ErrorName::ProxyConnectionFailed,
            "SOCKS5 proxy sent a different version",
        ));
    }
    match choice[1] {
        0x00 => {}
        0x02 => {
            let auth = auth.ok_or_else(|| {
                NetError::new(
                    ErrorName::TunnelConnectionFailed,
                    "SOCKS5 proxy asked for credentials",
                )
            })?;
            let mut message = vec![0x01, auth.username.len() as u8];
            message.extend_from_slice(auth.username.as_bytes());
            message.push(auth.password.len() as u8);
            message.extend_from_slice(auth.password.as_bytes());
            stream
                .write_all(&message)
                .await
                .map_err(|err| socks_error(&err))?;
            let mut reply = [0u8; 2];
            stream
                .read_exact(&mut reply)
                .await
                .map_err(|err| socks_error(&err))?;
            if reply[1] != 0x00 {
                return Err(NetError::new(
                    ErrorName::TunnelConnectionFailed,
                    "SOCKS5 credentials were refused",
                ));
            }
        }
        0xff => {
            return Err(NetError::new(
                ErrorName::TunnelConnectionFailed,
                "SOCKS5 proxy accepted none of the offered authentication methods",
            ));
        }
        other => {
            return Err(NetError::new(
                ErrorName::ProxyConnectionFailed,
                format!("SOCKS5 proxy chose unsupported method {other:#x}"),
            ));
        }
    }

    // CONNECT.
    let mut request = vec![0x05, 0x01, 0x00];
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => {
            request.push(0x01);
            request.extend_from_slice(&ip.octets());
        }
        Ok(std::net::IpAddr::V6(ip)) => {
            request.push(0x04);
            request.extend_from_slice(&ip.octets());
        }
        Err(_) => {
            if host.len() > 255 {
                return Err(NetError::new(
                    ErrorName::ProxyConnectionFailed,
                    "host name is too long for SOCKS5",
                ));
            }
            request.push(0x03);
            request.push(host.len() as u8);
            request.extend_from_slice(host.as_bytes());
        }
    }
    request.extend_from_slice(&port.to_be_bytes());
    stream
        .write_all(&request)
        .await
        .map_err(|err| socks_error(&err))?;

    let mut reply = [0u8; 4];
    stream
        .read_exact(&mut reply)
        .await
        .map_err(|err| socks_error(&err))?;
    if reply[1] != 0x00 {
        return Err(NetError::new(
            socks_reply_code(reply[1]),
            format!(
                "SOCKS5 proxy refused the connection (reply {:#x})",
                reply[1]
            ),
        ));
    }
    // Drain the bound address so the stream is positioned at the payload.
    match reply[3] {
        0x01 => {
            let mut rest = [0u8; 6];
            stream
                .read_exact(&mut rest)
                .await
                .map_err(|err| socks_error(&err))?;
        }
        0x04 => {
            let mut rest = [0u8; 18];
            stream
                .read_exact(&mut rest)
                .await
                .map_err(|err| socks_error(&err))?;
        }
        0x03 => {
            let mut length = [0u8; 1];
            stream
                .read_exact(&mut length)
                .await
                .map_err(|err| socks_error(&err))?;
            let mut rest = vec![0u8; length[0] as usize + 2];
            stream
                .read_exact(&mut rest)
                .await
                .map_err(|err| socks_error(&err))?;
        }
        other => {
            return Err(NetError::new(
                ErrorName::ProxyConnectionFailed,
                format!("SOCKS5 proxy sent unknown address type {other:#x}"),
            ));
        }
    }
    Ok(())
}

fn socks_reply_code(reply: u8) -> ErrorName {
    match reply {
        0x02 => ErrorName::AccessDenied,
        0x03 | 0x04 => ErrorName::AddressUnreachable,
        0x05 => ErrorName::ConnectionRefused,
        0x06 => ErrorName::ConnectionTimedOut,
        _ => ErrorName::ProxyConnectionFailed,
    }
}

fn tunnel_error(err: &std::io::Error) -> NetError {
    NetError::new(ErrorName::TunnelConnectionFailed, err.to_string())
}

fn socks_error(err: &std::io::Error) -> NetError {
    NetError::new(ErrorName::ProxyConnectionFailed, err.to_string())
}

/// `host:port`, bracketing IPv6 literals.
pub fn format_authority(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_urls_are_parsed_with_defaults() {
        let http = Proxy::parse("http://proxy.test").expect("proxy");
        assert_eq!(http.kind, ProxyKind::Http);
        assert_eq!(http.port, 8080);
        assert!(http.auth.is_none());

        let socks = Proxy::parse("socks5h://user:p%40ss@proxy.test:9050").expect("proxy");
        assert_eq!(socks.kind, ProxyKind::Socks5 { remote_dns: true });
        assert_eq!(socks.port, 9050);
        assert!(socks.resolves_remotely());
        let auth = socks.auth.as_ref().expect("auth");
        assert_eq!(auth.username, "user");
        assert_eq!(auth.password, "p@ss");

        assert_eq!(
            Proxy::parse("ftp://proxy.test").unwrap_err().name,
            ErrorName::InvalidArgument
        );
    }

    #[test]
    fn credentials_never_appear_in_debug_output_or_pool_keys() {
        let proxy = Proxy::parse("http://user:hunter2@proxy.test:3128").expect("proxy");
        let rendered = format!("{proxy:?}");
        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(!rendered.contains("user"), "{rendered}");
        assert_eq!(proxy.pool_key(), "http://proxy.test:3128");
    }

    #[test]
    fn authorities_bracket_ipv6() {
        assert_eq!(format_authority("example.com", 443), "example.com:443");
        assert_eq!(format_authority("::1", 8080), "[::1]:8080");
        assert_eq!(format_authority("[::1]", 8080), "[::1]:8080");
    }

    #[tokio::test]
    async fn http_connect_sends_basic_auth_and_reads_the_status() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let proxy = tokio::spawn(async move {
            let head = read_head(&mut server).await.expect("head");
            server
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .await
                .expect("write");
            head
        });
        let auth = ProxyAuth {
            username: "user".into(),
            password: "pass".into(),
        };
        http_connect(&mut client, "example.com", 443, Some(&auth), None)
            .await
            .expect("tunnel");
        let head = proxy.await.expect("join");
        assert!(
            head.starts_with("CONNECT example.com:443 HTTP/1.1\r\n"),
            "{head}"
        );
        assert!(
            head.contains("Proxy-Authorization: Basic dXNlcjpwYXNz\r\n"),
            "{head}"
        );
    }

    #[tokio::test]
    async fn a_refused_tunnel_maps_to_err_tunnel_connection_failed() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let _ = read_head(&mut server).await;
            let _ = server.write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n").await;
        });
        let err = http_connect(&mut client, "example.com", 443, None, None)
            .await
            .expect_err("must fail");
        assert_eq!(err.name, ErrorName::TunnelConnectionFailed);
    }

    #[tokio::test]
    async fn socks5_handshake_offers_credentials_and_sends_the_host_name() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let proxy = tokio::spawn(async move {
            let mut greeting = [0u8; 4];
            server.read_exact(&mut greeting).await.expect("greeting");
            server.write_all(&[0x05, 0x02]).await.expect("choice");

            let mut header = [0u8; 2];
            server.read_exact(&mut header).await.expect("auth header");
            let mut username = vec![0u8; header[1] as usize];
            server.read_exact(&mut username).await.expect("username");
            let mut password_len = [0u8; 1];
            server.read_exact(&mut password_len).await.expect("len");
            let mut password = vec![0u8; password_len[0] as usize];
            server.read_exact(&mut password).await.expect("password");
            server.write_all(&[0x01, 0x00]).await.expect("auth ok");

            let mut request = [0u8; 5];
            server.read_exact(&mut request).await.expect("request");
            let mut host = vec![0u8; request[4] as usize];
            server.read_exact(&mut host).await.expect("host");
            let mut port = [0u8; 2];
            server.read_exact(&mut port).await.expect("port");
            server
                .write_all(&[0x05, 0x00, 0x00, 0x01, 127, 0, 0, 1, 0x1f, 0x90])
                .await
                .expect("reply");
            (greeting, username, password, host, port)
        });

        let auth = ProxyAuth {
            username: "user".into(),
            password: "pass".into(),
        };
        socks5_connect(&mut client, "example.com", 443, Some(&auth))
            .await
            .expect("socks5");
        let (greeting, username, password, host, port) = proxy.await.expect("join");
        assert_eq!(greeting, [0x05, 0x02, 0x00, 0x02]);
        assert_eq!(username, b"user");
        assert_eq!(password, b"pass");
        assert_eq!(host, b"example.com");
        assert_eq!(port, 443u16.to_be_bytes());
    }

    #[tokio::test]
    async fn a_socks5_refusal_maps_to_the_matching_error() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let mut greeting = [0u8; 3];
            let _ = server.read_exact(&mut greeting).await;
            let _ = server.write_all(&[0x05, 0x00]).await;
            let mut request = [0u8; 10];
            let _ = server.read_exact(&mut request).await;
            let _ = server
                .write_all(&[0x05, 0x05, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await;
        });
        let err = socks5_connect(&mut client, "192.0.2.1", 80, None)
            .await
            .expect_err("must fail");
        assert_eq!(err.name, ErrorName::ConnectionRefused);
    }
}
