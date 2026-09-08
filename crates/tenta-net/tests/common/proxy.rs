//! Loopback proxies: an HTTP CONNECT tunnel and a SOCKS5 server, both of
//! which record the target they were asked for and then pipe bytes.

use std::net::SocketAddr;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// What the proxy was asked to reach, and with which credentials.
#[derive(Debug, Default)]
pub struct ProxyLog {
    targets: Mutex<Vec<String>>,
    credentials: Mutex<Vec<String>>,
}

impl ProxyLog {
    pub fn targets(&self) -> Vec<String> {
        self.targets.lock().clone()
    }

    pub fn credentials(&self) -> Vec<String> {
        self.credentials.lock().clone()
    }
}

pub struct ProxyServer {
    pub addr: SocketAddr,
    pub log: Arc<ProxyLog>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ProxyServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl ProxyServer {
    /// An HTTP proxy. With `required_auth` it answers `407` until the
    /// request carries exactly those `user:password` credentials.
    pub async fn http(required_auth: Option<&'static str>) -> Self {
        Self::start(move |socket, log| Box::pin(serve_connect(socket, log, required_auth))).await
    }

    /// A SOCKS5 proxy, optionally demanding username/password authentication.
    pub async fn socks5(required_auth: Option<&'static str>) -> Self {
        Self::start(move |socket, log| Box::pin(serve_socks5(socket, log, required_auth))).await
    }

    async fn start<F>(handler: F) -> Self
    where
        F: Fn(
                TcpStream,
                Arc<ProxyLog>,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
            + Send
            + Sync
            + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let log = Arc::new(ProxyLog::default());
        let served = Arc::clone(&log);
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                let log = Arc::clone(&served);
                let handler = Arc::clone(&handler);
                tokio::spawn(async move { handler(socket, log).await });
            }
        });
        Self { addr, log, task }
    }

    pub fn http_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.addr.port())
    }

    pub fn socks5_url(&self, remote_dns: bool) -> String {
        let scheme = if remote_dns { "socks5h" } else { "socks5" };
        format!("{scheme}://127.0.0.1:{}", self.addr.port())
    }
}

async fn serve_connect(mut socket: TcpStream, log: Arc<ProxyLog>, required_auth: Option<&str>) {
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let Ok(read) = socket.read(&mut chunk).await else {
            return;
        };
        if read == 0 {
            return;
        }
        head.extend_from_slice(&chunk[..read]);
        if head.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    let Some(target) = text
        .lines()
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .map(str::to_owned)
    else {
        return;
    };
    log.targets.lock().push(target.clone());

    if let Some(expected) = required_auth {
        let offered = text
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("proxy-authorization"))
            .map(|(_, value)| value.trim().to_owned());
        match offered {
            Some(value) => {
                let decoded = value
                    .strip_prefix("Basic ")
                    .and_then(|encoded| {
                        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
                            .ok()
                    })
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .unwrap_or_default();
                log.credentials.lock().push(decoded.clone());
                if decoded != expected {
                    let _ = socket
                        .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n")
                        .await;
                    return;
                }
            }
            None => {
                let _ = socket
                    .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n")
                    .await;
                return;
            }
        }
    }

    let Ok(mut upstream) = TcpStream::connect(&target).await else {
        let _ = socket.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
        return;
    };
    if socket
        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .await
        .is_err()
    {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut socket, &mut upstream).await;
}

async fn serve_socks5(mut socket: TcpStream, log: Arc<ProxyLog>, required_auth: Option<&str>) {
    let mut greeting = [0u8; 2];
    if socket.read_exact(&mut greeting).await.is_err() || greeting[0] != 0x05 {
        return;
    }
    let mut methods = vec![0u8; usize::from(greeting[1])];
    if socket.read_exact(&mut methods).await.is_err() {
        return;
    }

    if required_auth.is_some() {
        if !methods.contains(&0x02) {
            let _ = socket.write_all(&[0x05, 0xff]).await;
            return;
        }
        if socket.write_all(&[0x05, 0x02]).await.is_err() {
            return;
        }
        let mut version = [0u8; 2];
        if socket.read_exact(&mut version).await.is_err() {
            return;
        }
        let mut username = vec![0u8; usize::from(version[1])];
        if socket.read_exact(&mut username).await.is_err() {
            return;
        }
        let mut password_len = [0u8; 1];
        if socket.read_exact(&mut password_len).await.is_err() {
            return;
        }
        let mut password = vec![0u8; usize::from(password_len[0])];
        if socket.read_exact(&mut password).await.is_err() {
            return;
        }
        let offered = format!(
            "{}:{}",
            String::from_utf8_lossy(&username),
            String::from_utf8_lossy(&password)
        );
        log.credentials.lock().push(offered.clone());
        let accepted = required_auth == Some(offered.as_str());
        let _ = socket
            .write_all(&[0x01, if accepted { 0x00 } else { 0x01 }])
            .await;
        if !accepted {
            return;
        }
    } else if socket.write_all(&[0x05, 0x00]).await.is_err() {
        return;
    }

    let mut request = [0u8; 4];
    if socket.read_exact(&mut request).await.is_err() || request[1] != 0x01 {
        return;
    }
    let host = match request[3] {
        0x01 => {
            let mut octets = [0u8; 4];
            if socket.read_exact(&mut octets).await.is_err() {
                return;
            }
            std::net::Ipv4Addr::from(octets).to_string()
        }
        0x03 => {
            let mut length = [0u8; 1];
            if socket.read_exact(&mut length).await.is_err() {
                return;
            }
            let mut name = vec![0u8; usize::from(length[0])];
            if socket.read_exact(&mut name).await.is_err() {
                return;
            }
            String::from_utf8_lossy(&name).into_owned()
        }
        _ => return,
    };
    let mut port = [0u8; 2];
    if socket.read_exact(&mut port).await.is_err() {
        return;
    }
    let port = u16::from_be_bytes(port);
    log.targets.lock().push(format!("{host}:{port}"));

    // The test proxy only reaches loopback, whatever name it was given.
    let Ok(mut upstream) = TcpStream::connect(("127.0.0.1", port)).await else {
        let _ = socket
            .write_all(&[0x05, 0x01, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
            .await;
        return;
    };
    if socket
        .write_all(&[0x05, 0x00, 0x00, 0x01, 127, 0, 0, 1, 0, 0])
        .await
        .is_err()
    {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut socket, &mut upstream).await;
}
