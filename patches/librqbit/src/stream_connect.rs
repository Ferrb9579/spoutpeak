use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{Context, bail};
use librqbit_dualstack_sockets::ConnectOpts;
use librqbit_utp::{BindDevice, UtpSocketUdp};
use serde::Serialize;
use tracing::debug;

use crate::{
    Error, PeerConnectionOptions, Result,
    type_aliases::{BoxAsyncReadVectored, BoxAsyncWrite},
    vectored_traits::AsyncReadVectoredIntoCompat,
};

#[derive(Debug, Clone, Copy, Serialize)]
pub enum ConnectionKind {
    #[serde(rename = "tcp")]
    Tcp,
    #[serde(rename = "utp")]
    Utp,
    #[serde(rename = "socks")]
    Socks,
}

impl std::fmt::Display for ConnectionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionKind::Tcp => f.write_str("tcp"),
            ConnectionKind::Utp => f.write_str("uTP"),
            ConnectionKind::Socks => f.write_str("socks"),
        }
    }
}

pub struct ConnectionOptions {
    // socks5://[username:password@]host:port
    // If set, all outgoing connections will go through the proxy over TCP.
    pub proxy_url: Option<String>,
    // TCP outgoing connections are enabled by default
    pub enable_tcp: bool,
    pub peer_opts: Option<PeerConnectionOptions>,
}

impl Default for ConnectionOptions {
    fn default() -> Self {
        Self {
            enable_tcp: true,
            proxy_url: None,
            peer_opts: None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SocksProxyConfig {
    pub host: String,
    pub port: u16,
    pub username_password: Option<(String, String)>,
}

#[derive(Default, Debug, Clone)]
pub(crate) struct StreamConnectorArgs {
    pub encryption: crate::EncryptionMode,
    pub enable_tcp: bool,
    pub socks_proxy_config: Option<SocksProxyConfig>,
    pub utp_socket: Option<Arc<UtpSocketUdp>>,
    pub bind_device: Option<BindDevice>,
    pub ipv4_only: bool,
}

impl SocksProxyConfig {
    /// socks5h:// resolves hostnames via the proxy; socks5:// resolves them locally.
    pub fn parse(url: &str) -> anyhow::Result<Self> {
        let url = ::url::Url::parse(url).context("invalid proxy URL")?;
        match url.scheme() {
            "socks5" | "socks5h" => {}
            other => anyhow::bail!("unsupported proxy URL scheme \"{other}\""),
        }
        let host = url.host_str().context("missing host")?;
        let port = url.port().context("missing port")?;
        let up = url
            .password()
            .map(|p| (url.username().to_owned(), p.to_owned()));
        Ok(Self {
            host: host.to_owned(),
            port,
            username_password: up,
        })
    }

    async fn connect(
        &self,
        addr: SocketAddr,
    ) -> tokio_socks::Result<(
        impl tokio::io::AsyncRead + Unpin + 'static,
        impl tokio::io::AsyncWrite + Unpin + 'static,
    )> {
        let proxy_addr = (self.host.as_str(), self.port);

        let stream = if let Some((username, password)) = self.username_password.as_ref() {
            tokio_socks::tcp::Socks5Stream::connect_with_password(
                proxy_addr,
                addr,
                username.as_str(),
                password.as_str(),
            )
            .await?
        } else {
            tokio_socks::tcp::Socks5Stream::connect(proxy_addr, addr).await?
        };

        Ok(tokio::io::split(stream))
    }
}

gen_stats!(SingleStatAtomic SingleStatSnapshot, [
    attempts u64,
    successes u64,
    errors u64
], []);
gen_stats!(PerFamilyAtomic PerFamilySnapshot, [], [
    v4 SingleStatAtomic SingleStatSnapshot,
    v6 SingleStatAtomic SingleStatSnapshot
]);
gen_stats!(ConnectStatsAtomic ConnectStatsSnapshot, [], [
    socks PerFamilyAtomic PerFamilySnapshot,
    tcp PerFamilyAtomic PerFamilySnapshot,
    utp PerFamilyAtomic PerFamilySnapshot
]);

#[derive(Debug)]
pub(crate) struct StreamConnector {
    encryption: tokio::sync::watch::Sender<crate::EncryptionMode>,
    proxy_config: Option<SocksProxyConfig>,
    enable_tcp: bool,
    bind_device: Option<BindDevice>,
    utp_socket: Option<Arc<librqbit_utp::UtpSocketUdp>>,
    stats: ConnectStatsAtomic,
    ipv4_only: bool,
}

impl StreamConnector {
    pub async fn new(config: StreamConnectorArgs) -> anyhow::Result<Self> {
        #[allow(clippy::single_match)]
        match (
            config.socks_proxy_config.is_some(),
            config.enable_tcp,
            config.utp_socket.is_some(),
        ) {
            (false, false, false) => {
                bail!("no way to connect to peers, enable TCP, uTP or socks proxy")
            }
            _ => {
                // TODO: maybe validate other combinations. For now there's no way to disable TCP
            }
        }

        Ok(Self {
            encryption: tokio::sync::watch::channel(config.encryption).0,
            proxy_config: config.socks_proxy_config,
            enable_tcp: config.enable_tcp,
            utp_socket: config.utp_socket,
            bind_device: config.bind_device,
            stats: Default::default(),
            ipv4_only: config.ipv4_only,
        })
    }

    fn get_stat(&self, kind: ConnectionKind, is_v6: bool) -> &SingleStatAtomic {
        let stat = match kind {
            ConnectionKind::Tcp => &self.stats.tcp,
            ConnectionKind::Utp => &self.stats.utp,
            ConnectionKind::Socks => &self.stats.socks,
        };
        if is_v6 { &stat.v6 } else { &stat.v4 }
    }

    async fn with_stat<R, E>(
        &self,
        kind: ConnectionKind,
        is_v6: bool,
        fut: impl Future<Output = std::result::Result<R, E>>,
    ) -> std::result::Result<R, E> {
        let stat = self.get_stat(kind, is_v6);
        stat.attempts(1);
        fut.await
            .inspect(|_| stat.successes(1))
            .inspect_err(|_| stat.errors(1))
    }

    async fn tcp_connect(
        &self,
        addr: SocketAddr,
    ) -> librqbit_dualstack_sockets::Result<tokio::net::TcpStream> {
        self.with_stat(
            ConnectionKind::Tcp,
            addr.is_ipv6(),
            librqbit_dualstack_sockets::tcp_connect(
                addr,
                ConnectOpts {
                    // Setting source port doesn't work with cloudflare warp on linux
                    // source_port: self.tcp_source_port,
                    source_port: None,
                    bind_device: self.bind_device.as_ref(),
                },
            ),
        )
        .await
    }

    pub fn stats(&self) -> &ConnectStatsAtomic {
        &self.stats
    }

    pub(crate) fn encryption(&self) -> crate::EncryptionMode {
        *self.encryption.borrow()
    }

    pub(crate) fn subscribe_encryption(
        &self,
    ) -> tokio::sync::watch::Receiver<crate::EncryptionMode> {
        self.encryption.subscribe()
    }

    pub(crate) fn set_encryption(&self, mode: crate::EncryptionMode) {
        self.encryption.send_if_modified(|current| {
            if *current == mode {
                return false;
            }
            *current = mode;
            true
        });
    }

    pub(crate) async fn connect_peer(
        &self,
        addr: SocketAddr,
        key: librqbit_core::Id20,
        timeout: Duration,
    ) -> Result<(ConnectionKind, BoxAsyncReadVectored, BoxAsyncWrite)> {
        let mode = self.encryption();
        if mode != crate::EncryptionMode::Disabled {
            let encrypted = tokio::time::timeout(timeout, async {
                let (kind, r, w) = self.connect(addr).await?;
                let (r, w) = crate::mse::initiate(r, w, key)
                    .await
                    .map_err(Error::Anyhow)?;
                Ok::<_, Error>((kind, r, w))
            })
            .await;
            match encrypted {
                Ok(Ok(connection)) => return Ok(connection),
                _ if mode == crate::EncryptionMode::Prefer => {
                    debug!(
                        ?addr,
                        "MSE failed; retrying with a fresh plaintext connection"
                    );
                }
                Ok(Err(error)) => return Err(error),
                Err(error) => return Err(Error::Anyhow(error.into())),
            }
        }
        tokio::time::timeout(timeout, self.connect(addr))
            .await
            .map_err(|e| Error::Anyhow(e.into()))?
    }

    pub async fn connect(
        &self,
        addr: SocketAddr,
    ) -> Result<(ConnectionKind, BoxAsyncReadVectored, BoxAsyncWrite)> {
        if addr.port() == 0 {
            return Err(Error::Anyhow(anyhow::anyhow!(
                "invalid peer address (port 0): {}",
                addr
            )));
        }

        if self.ipv4_only && addr.is_ipv6() {
            return Err(Error::Anyhow(anyhow::anyhow!(
                "ipv6 disabled, skipping connection to {}",
                addr
            )));
        }

        if let Some(proxy) = self.proxy_config.as_ref() {
            let (r, w) = self
                .with_stat(ConnectionKind::Socks, addr.is_ipv6(), proxy.connect(addr))
                .await?;
            debug!(?addr, "connected through SOCKS5");
            return Ok((
                ConnectionKind::Socks,
                Box::new(r.into_vectored_compat()),
                Box::new(w),
            ));
        }

        // Try to connect over TCP first. If in 1 second we haven't connected, try uTP also (if configured).
        // Whoever connects first wins.
        let tcp_connect = async {
            if !self.enable_tcp {
                return Ok(None);
            }
            let conn = self.tcp_connect(addr).await?;
            debug!(?addr, "connected over TCP");
            Ok::<_, librqbit_dualstack_sockets::Error>(Some(conn))
        };

        let tcp_failed_notify = tokio::sync::Notify::new();

        let utp_connect = async {
            let sock = match self.utp_socket.as_ref() {
                Some(sock) => sock,
                None => return Ok(None),
            };

            // Give TCP priority as it's more mature and simpler.
            if self.enable_tcp {
                // wait until either 1 second has passed or TCP failed.
                tokio::select! {
                    _ = tcp_failed_notify.notified() => {},
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
            }

            let conn = self
                .with_stat(ConnectionKind::Utp, addr.is_ipv6(), sock.connect(addr))
                .await?;

            debug!(?addr, "connected over uTP");
            Ok(Some(conn))
        };

        tokio::pin!(tcp_connect);
        tokio::pin!(utp_connect);

        let mut tcp_err: Option<Option<librqbit_dualstack_sockets::Error>> = None;
        let mut utp_err: Option<Option<librqbit_utp::Error>> = None;

        // wait until all fail, or one succeeds.
        loop {
            if let (Some(tcp), Some(utp)) = (tcp_err.as_mut(), utp_err.as_mut()) {
                match (tcp.take(), utp.take()) {
                    (Some(tcp), Some(utp)) => return Err(Error::Connect { tcp, utp }),
                    (Some(tcp), None) => return Err(Error::TcpConnect(tcp)),
                    (None, Some(utp)) => return Err(Error::UtpConnect(utp)),
                    (None, None) => return Err(Error::ConnectDisabled),
                }
            }
            tokio::select! {
                tcp_res = &mut tcp_connect, if tcp_err.is_none() => {
                    match tcp_res {
                        Ok(Some(stream)) => {
                            let (r, w) = stream.into_split();
                            return Ok((ConnectionKind::Tcp, Box::new(r), Box::new(w)));
                        },
                        Ok(None) => {
                            tcp_err = Some(None);
                            tcp_failed_notify.notify_waiters();
                        }
                        Err(e) => {
                            tcp_err = Some(Some(e));
                            tcp_failed_notify.notify_waiters();
                        }
                    }
                },
                utp_res = &mut utp_connect, if utp_err.is_none() => {
                    match utp_res {
                        Ok(Some(stream)) => {
                            let (r, w) = stream.split();
                            return Ok((ConnectionKind::Utp, Box::new(r), Box::new(w)));
                        },
                        Ok(None) => {
                            utp_err = Some(None);
                        }
                        Err(e) => {
                            utp_err = Some(Some(e));
                        }
                    }
                },
            };
        }
    }
}

#[cfg(test)]
mod encryption_tests {
    use super::*;
    use crate::EncryptionMode;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn prefer_retries_on_a_fresh_connection_and_require_never_downgrades() {
        for mode in [
            EncryptionMode::Prefer,
            EncryptionMode::Require,
            EncryptionMode::Disabled,
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let connector = StreamConnector::new(StreamConnectorArgs {
                encryption: mode,
                enable_tcp: true,
                ..Default::default()
            })
            .await
            .unwrap();
            let server = async {
                let (mut first, _) = listener.accept().await.unwrap();
                if mode != EncryptionMode::Disabled {
                    let mut public = [0; 96];
                    first.read_exact(&mut public).await.unwrap();
                    drop(first); // simulate a peer without MSE support
                    if mode == EncryptionMode::Require {
                        assert!(
                            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                                .await
                                .is_err()
                        );
                        return;
                    }
                    first = listener.accept().await.unwrap().0;
                }
                let mut header = [0; 20];
                first.read_exact(&mut header).await.unwrap();
                assert_eq!(&header, b"\x13BitTorrent protocol");
            };
            let client = async {
                let result = connector
                    .connect_peer(
                        addr,
                        librqbit_core::Id20::new([1; 20]),
                        Duration::from_secs(1),
                    )
                    .await;
                if mode == EncryptionMode::Require {
                    assert!(result.is_err());
                } else {
                    let (_, _, mut w) = result.unwrap();
                    w.write_all(b"\x13BitTorrent protocol").await.unwrap();
                }
            };
            tokio::time::timeout(Duration::from_secs(5), async {
                tokio::join!(server, client);
            })
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn stalled_encryption_times_out_before_plaintext_retry() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let connector = StreamConnector::new(StreamConnectorArgs {
            encryption: EncryptionMode::Prefer,
            enable_tcp: true,
            ..Default::default()
        })
        .await
        .unwrap();
        let server = async {
            let (_stalled, _) = listener.accept().await.unwrap();
            let (mut retry, _) = listener.accept().await.unwrap();
            let mut marker = [0; 5];
            retry.read_exact(&mut marker).await.unwrap();
            assert_eq!(&marker, b"plain");
        };
        let client = async {
            let (_, _, mut w) = connector
                .connect_peer(
                    addr,
                    librqbit_core::Id20::new([1; 20]),
                    Duration::from_millis(100),
                )
                .await
                .unwrap();
            w.write_all(b"plain").await.unwrap();
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(server, client);
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn policy_changes_notify_active_connections_but_noop_updates_do_not() {
        let connector = StreamConnector::new(StreamConnectorArgs {
            enable_tcp: true,
            ..Default::default()
        })
        .await
        .unwrap();
        let mut policy = connector.subscribe_encryption();
        connector.set_encryption(EncryptionMode::Disabled);
        assert!(!policy.has_changed().unwrap());
        connector.set_encryption(EncryptionMode::Require);
        policy.changed().await.unwrap();
        assert_eq!(*policy.borrow_and_update(), EncryptionMode::Require);
        connector.set_encryption(EncryptionMode::Prefer);
        assert!(policy.has_changed().unwrap());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_socks5() {
        let c = SocksProxyConfig::parse("socks5://myhost:1234").unwrap();
        assert_eq!(c.host, "myhost");
        assert_eq!(c.port, 1234);
        assert_eq!(c.username_password, None);
    }

    #[test]
    fn test_parse_socks5h() {
        let c = SocksProxyConfig::parse("socks5h://myhost:1234").unwrap();
        assert_eq!(c.host, "myhost");
        assert_eq!(c.port, 1234);
    }

    #[test]
    fn test_parse_with_password() {
        let c = SocksProxyConfig::parse("socks5h://user:pass@myhost:1234").unwrap();
        assert_eq!(c.host, "myhost");
        assert_eq!(c.port, 1234);
        assert_eq!(
            c.username_password,
            Some(("user".to_owned(), "pass".to_owned()))
        );
    }

    #[test]
    fn test_parse_rejects_other_schemes() {
        for url in [
            "http://myhost:1234",
            "socks4://myhost:1234",
            "socks4a://myhost:1234",
        ] {
            assert!(SocksProxyConfig::parse(url).is_err(), "should reject {url}");
        }
    }

    #[test]
    fn test_parse_rejects_garbage() {
        assert!(SocksProxyConfig::parse("not a url").is_err());
        assert!(SocksProxyConfig::parse("socks5://").is_err());
    }
}
