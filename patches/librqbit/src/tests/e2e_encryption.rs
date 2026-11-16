use std::{path::Path, time::Duration};

use crate::{
    AddTorrent, AddTorrentOptions, ConnectionOptions, CreateTorrentOptions, EncryptionMode,
    ListenerMode, ListenerOptions, Session, SessionOptions,
};

fn options(transport: ListenerMode, encryption: EncryptionMode) -> SessionOptions {
    SessionOptions {
        encryption,
        dht: None,
        disable_trackers: true,
        disable_local_service_discovery: true,
        listen: Some(ListenerOptions {
            mode: transport,
            listen_addr: "127.0.0.1:0".parse().unwrap(),
            enable_upnp_port_forwarding: false,
            ..Default::default()
        }),
        connect: Some(ConnectionOptions {
            enable_tcp: transport.tcp_enabled(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

async fn seed(
    root: &Path,
    transport: ListenerMode,
) -> (std::sync::Arc<Session>, crate::CreateTorrentResult, Vec<u8>) {
    std::fs::create_dir_all(root.join("seed")).unwrap();
    let payload: Vec<_> = (0..256_123).map(|i| (i % 251) as u8).collect();
    let path = root.join("seed/payload.bin");
    std::fs::write(&path, &payload).unwrap();
    let session = Session::new_with_opts(
        root.join("seed"),
        options(transport, EncryptionMode::Require),
    )
    .await
    .unwrap();
    let (torrent, handle) = session
        .create_and_serve_torrent(
            &path,
            CreateTorrentOptions {
                piece_length: Some(32768),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    handle.wait_until_completed().await.unwrap();
    (session, torrent, payload)
}

#[tokio::test(flavor = "multi_thread")]
async fn encrypted_magnet_metadata_and_download_over_tcp_and_utp() {
    for transport in [ListenerMode::TcpOnly, ListenerMode::UtpOnly] {
        tokio::time::timeout(Duration::from_secs(20), async {
            let root = tempfile::tempdir().unwrap();
            let (server, torrent, payload) = seed(root.path(), transport).await;
            let client = Session::new_with_opts(
                root.path().join("download"),
                options(transport, EncryptionMode::Require),
            )
            .await
            .unwrap();
            let handle = client
                .add_torrent(
                    AddTorrent::from_url(torrent.as_magnet().to_string()),
                    Some(AddTorrentOptions {
                        initial_peers: Some(vec![server.listen_addr().unwrap()]),
                        ..Default::default()
                    }),
                )
                .await
                .unwrap()
                .into_handle()
                .unwrap();
            handle.wait_until_completed().await.unwrap();
            assert_eq!(
                std::fs::read(root.path().join("download/payload.bin")).unwrap(),
                payload
            );
            client.stop().await;
            server.stop().await;
        })
        .await
        .unwrap();
    }
}

struct ChildGuard(std::process::Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn wait_file(path: &Path, child: &mut ChildGuard) {
    while !path.exists() {
        if let Some(status) = child.0.try_wait().unwrap() {
            panic!("libtorrent exited before {}: {status}", path.display());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Optional independent-client test. Install Python libtorrent and set
/// SPOUTPEAK_LIBTORRENT_PYTHONPATH if installed outside Python's normal path.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Python with the libtorrent module"]
async fn libtorrent_interoperability() {
    for transport in [ListenerMode::TcpOnly, ListenerMode::UtpOnly] {
        for role in ["seed", "download"] {
            tokio::time::timeout(Duration::from_secs(30), async {
                let root = tempfile::tempdir().unwrap();
                let (server, torrent, payload) = seed(root.path(), transport).await;
                std::fs::write(
                    root.path().join("payload.torrent"),
                    torrent.as_bytes().unwrap(),
                )
                .unwrap();
                let mut command = std::process::Command::new(
                    std::env::var("SPOUTPEAK_LIBTORRENT_PYTHON")
                        .unwrap_or_else(|_| "python".to_owned()),
                );
                command.args(["-c", include_str!("libtorrent_encryption_peer.py")]);
                command
                    .arg(root.path())
                    .arg(role)
                    .arg(if transport.tcp_enabled() {
                        "tcp"
                    } else {
                        "utp"
                    });
                if let Ok(path) = std::env::var("SPOUTPEAK_LIBTORRENT_PYTHONPATH") {
                    command.env("PYTHONPATH", path);
                }
                let mut child = ChildGuard(command.spawn().unwrap());
                wait_file(&root.path().join("ready.json"), &mut child).await;
                let ready: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(root.path().join("ready.json")).unwrap())
                        .unwrap();
                if role == "seed" {
                    let client = Session::new_with_opts(
                        root.path().join("download"),
                        options(transport, EncryptionMode::Require),
                    )
                    .await
                    .unwrap();
                    let peer = format!("127.0.0.1:{}", ready["port"].as_u64().unwrap())
                        .parse()
                        .unwrap();
                    let handle = client
                        .add_torrent(
                            AddTorrent::from_url(torrent.as_magnet().to_string()),
                            Some(AddTorrentOptions {
                                initial_peers: Some(vec![peer]),
                                ..Default::default()
                            }),
                        )
                        .await
                        .unwrap()
                        .into_handle()
                        .unwrap();
                    handle.wait_until_completed().await.unwrap();
                    assert_eq!(
                        std::fs::read(root.path().join("download/payload.bin")).unwrap(),
                        payload
                    );
                    wait_file(&root.path().join("encrypted"), &mut child).await;
                    client.stop().await;
                } else {
                    std::fs::write(
                        root.path().join("connect.json"),
                        serde_json::to_vec(&server.listen_addr().unwrap().port()).unwrap(),
                    )
                    .unwrap();
                    wait_file(&root.path().join("completed"), &mut child).await;
                    assert_eq!(
                        std::fs::read(root.path().join("python-download/payload.bin")).unwrap(),
                        payload
                    );
                }
                std::fs::write(root.path().join("stop"), b"").unwrap();
                server.stop().await;
                println!("libtorrent {role}, {transport:?}: verified RC4 and complete payload");
            })
            .await
            .unwrap();
        }
    }
}
