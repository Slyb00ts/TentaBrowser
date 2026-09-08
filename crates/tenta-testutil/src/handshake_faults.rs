//! A loopback listener that breaks a TLS handshake the way a hostile or
//! overloaded origin does.
//!
//! Every real search provider and every real site is HTTPS, so a client that
//! reports a TCP reset as a certificate failure loses the retry class the
//! caller needs (`docs/spec/03-network.md` §7, `docs/spec/08-search.md` §4).
//! Testing that needs a peer that fails *below* TLS, which is why this fixture
//! speaks no TLS at all: it accepts the connection and then either resets it
//! or closes it cleanly while the client is still in its handshake.

use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

/// How the fixture breaks the handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandshakeFault {
    /// Accept and then reset the connection with the ClientHello still
    /// unread, which is what a middlebox or a blocking provider does. Closing
    /// a socket with unread data in its receive buffer sends `RST`.
    Reset,
    /// Accept, drain the ClientHello and close cleanly: the client sees EOF
    /// where it expected a ServerHello.
    Close,
}

/// A listener that fails every connection it accepts.
#[derive(Debug)]
pub struct HandshakeFaultServer {
    addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl HandshakeFaultServer {
    /// Binds a loopback port and starts accepting.
    pub async fn start(fault: HandshakeFault) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                match fault {
                    HandshakeFault::Reset => {
                        // Let the ClientHello land in the receive buffer, then
                        // drop the socket without reading it.
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        drop(stream);
                    }
                    HandshakeFault::Close => {
                        let mut scratch = [0u8; 4096];
                        let _ = stream.read(&mut scratch).await;
                        let _ = tokio::io::AsyncWriteExt::shutdown(&mut stream).await;
                        drop(stream);
                    }
                }
            }
        });
        Ok(Self { addr, task })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// The `https://` URL of `path` on this listener.
    pub fn url(&self, path: &str) -> String {
        format!("https://127.0.0.1:{}{path}", self.addr.port())
    }
}

impl Drop for HandshakeFaultServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
