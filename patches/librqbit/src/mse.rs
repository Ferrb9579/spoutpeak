//! BitTorrent MSE/PE transport (768-bit DH, SHA-1 and RC4-drop1024).
//! This is protocol obfuscation, not an authenticated secure transport.
//! Wire reference: https://wiki.vuze.com/w/Message_Stream_Encryption

use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use anyhow::{bail, ensure};
use librqbit_core::Id20;
use num_bigint::BigUint;
use rc4::{
    Rc4,
    cipher::{KeyInit, StreamCipher, consts::U20},
};
use serde::{Deserialize, Serialize};
use sha1w::{ISha1, Sha1};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::{
    type_aliases::{BoxAsyncReadVectored, BoxAsyncWrite},
    vectored_traits::AsyncReadVectoredIntoCompat,
};

const MAX_PAD: usize = 512;
const PLAIN_HEADER: &[u8; 20] = b"\x13BitTorrent protocol";
const PRIME: &[u8] = b"FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A63A36210000000000090563";
type Cipher = Rc4<U20>;
type Transport = (BoxAsyncReadVectored, BoxAsyncWrite);

/// Encryption policy for incoming and outgoing peer payloads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EncryptionMode {
    /// Only ordinary plaintext BitTorrent connections.
    #[default]
    Disabled,
    /// Try RC4 first, with a fresh plaintext connection as fallback.
    Prefer,
    /// Only RC4 payloads; never fall back to plaintext.
    Require,
}

fn hash(parts: &[&[u8]]) -> [u8; 20] {
    let mut h = Sha1::new();
    for part in parts {
        h.update(part);
    }
    h.finish()
}

fn fixed_key(n: &BigUint) -> [u8; 96] {
    let bytes = n.to_bytes_be();
    let mut out = [0; 96];
    out[96 - bytes.len()..].copy_from_slice(&bytes);
    out
}

struct Exchange {
    private: BigUint,
    prime: BigUint,
}
impl Exchange {
    fn new() -> anyhow::Result<Self> {
        let mut secret = [0; 20];
        getrandom::fill(&mut secret).map_err(|e| anyhow::anyhow!("MSE entropy: {e}"))?;
        // Keep even a degenerate RNG output away from exponent zero or one.
        secret[0] |= 0x80;
        Ok(Self {
            private: BigUint::from_bytes_be(&secret),
            prime: BigUint::parse_bytes(PRIME, 16).unwrap(),
        })
    }
    fn public(&self) -> [u8; 96] {
        fixed_key(&BigUint::from(2u8).modpow(&self.private, &self.prime))
    }
    fn shared(&self, public: &[u8; 96]) -> anyhow::Result<[u8; 96]> {
        let n = BigUint::from_bytes_be(public);
        ensure!(
            n >= BigUint::from(2u8) && n < &self.prime - BigUint::from(1u8),
            "invalid MSE DH public key"
        );
        Ok(fixed_key(&n.modpow(&self.private, &self.prime)))
    }
}

fn cipher(label: &[u8], secret: &[u8; 96], key: Id20) -> Cipher {
    let mut c = Cipher::new(&hash(&[label, secret, &key.0]).into());
    c.apply_keystream(&mut [0; 1024]);
    c
}

async fn public_key(w: &mut BoxAsyncWrite, exchange: &Exchange) -> anyhow::Result<()> {
    let mut random = [0; MAX_PAD + 2];
    getrandom::fill(&mut random).map_err(|e| anyhow::anyhow!("MSE entropy: {e}"))?;
    let len = usize::from(u16::from_be_bytes([random[0], random[1]])) % (MAX_PAD + 1);
    let mut bytes = exchange.public().to_vec();
    bytes.extend_from_slice(&random[2..2 + len]);
    w.write_all(&bytes).await?;
    w.flush().await?;
    Ok(())
}

// Read exactly through the synchronization marker, leaving subsequent bytes
// (including coalesced payload) in the transport. Padding is bounded by MSE.
async fn sync(r: &mut BoxAsyncReadVectored, marker: &[u8]) -> anyhow::Result<()> {
    let mut window = vec![0; marker.len()];
    r.read_exact(&mut window).await?;
    for skipped in 0..=MAX_PAD {
        if window == marker {
            return Ok(());
        }
        ensure!(skipped < MAX_PAD, "MSE synchronization padding too long");
        window.rotate_left(1);
        r.read_exact(&mut window[marker.len() - 1..]).await?;
    }
    unreachable!()
}

async fn decrypt(
    r: &mut BoxAsyncReadVectored,
    c: &mut Cipher,
    len: usize,
) -> anyhow::Result<Vec<u8>> {
    let mut bytes = vec![0; len];
    r.read_exact(&mut bytes).await?;
    c.apply_keystream(&mut bytes);
    Ok(bytes)
}

async fn padding(r: &mut BoxAsyncReadVectored, c: &mut Cipher, len: &[u8]) -> anyhow::Result<()> {
    let len = usize::from(u16::from_be_bytes(len.try_into()?));
    ensure!(len <= MAX_PAD, "MSE padding too long");
    decrypt(r, c, len).await?;
    Ok(())
}

/// Initiator: offer RC4 only, so Prefer never silently selects plaintext
/// inside MSE. The connector owns the separate plaintext retry.
pub(crate) async fn initiate(
    mut r: BoxAsyncReadVectored,
    mut w: BoxAsyncWrite,
    key: Id20,
) -> anyhow::Result<Transport> {
    let exchange = Exchange::new()?;
    public_key(&mut w, &exchange).await?;
    let mut remote = [0; 96];
    r.read_exact(&mut remote).await?;
    let secret = exchange.shared(&remote)?;
    let mut encrypt = cipher(b"keyA", &secret, key);
    let mut decryptor = cipher(b"keyB", &secret, key);
    let mut request = hash(&[b"req1", &secret]).to_vec();
    let req2 = hash(&[b"req2", &key.0]);
    let req3 = hash(&[b"req3", &secret]);
    request.extend(req2.iter().zip(req3).map(|(a, b)| a ^ b));
    // VC, crypto_provide=RC4, len(PadC)=0, len(IA)=0.
    let mut encrypted = vec![0; 16];
    encrypted[11] = 2;
    encrypt.apply_keystream(&mut encrypted);
    request.extend(encrypted);
    w.write_all(&request).await?;
    w.flush().await?;
    let mut vc = [0; 8];
    decryptor.apply_keystream(&mut vc);
    sync(&mut r, &vc).await?;
    let response = decrypt(&mut r, &mut decryptor, 6).await?;
    ensure!(
        &response[..4] == [0, 0, 0, 2],
        "peer did not select MSE RC4"
    );
    padding(&mut r, &mut decryptor, &response[4..]).await?;
    Ok(wrap(r, w, Some(decryptor), Some(encrypt), Vec::new()))
}

/// Responder: identify the torrent by its obfuscated req2 hash before reading
/// the peer handshake, and preserve any initial payload (IA).
pub(crate) async fn accept(
    mut r: BoxAsyncReadVectored,
    mut w: BoxAsyncWrite,
    mode: EncryptionMode,
    keys: &[Id20],
) -> anyhow::Result<(BoxAsyncReadVectored, BoxAsyncWrite, Option<Id20>)> {
    let mut first = [0; 20];
    r.read_exact(&mut first).await?;
    if &first == PLAIN_HEADER {
        ensure!(
            mode != EncryptionMode::Require,
            "plaintext peer rejected: encryption required"
        );
        let (r, w) = wrap(r, w, None, None, first.to_vec());
        return Ok((r, w, None));
    }
    ensure!(mode != EncryptionMode::Disabled, "MSE disabled");
    let mut remote = [0; 96];
    remote[..20].copy_from_slice(&first);
    r.read_exact(&mut remote[20..]).await?;
    let exchange = Exchange::new()?;
    let secret = exchange.shared(&remote)?;
    public_key(&mut w, &exchange).await?;
    sync(&mut r, &hash(&[b"req1", &secret])).await?;
    let mut req2 = [0; 20];
    r.read_exact(&mut req2).await?;
    for (byte, mask) in req2.iter_mut().zip(hash(&[b"req3", &secret])) {
        *byte ^= mask;
    }
    let key = keys
        .iter()
        .copied()
        .find(|key| hash(&[b"req2", &key.0]) == req2)
        .ok_or_else(|| anyhow::anyhow!("unknown MSE torrent"))?;
    let mut decryptor = cipher(b"keyA", &secret, key);
    let mut encrypt = cipher(b"keyB", &secret, key);
    let header = decrypt(&mut r, &mut decryptor, 14).await?;
    ensure!(header[..8] == [0; 8], "invalid MSE verification constant");
    let provided = u32::from_be_bytes(header[8..12].try_into()?);
    let selected = if provided & 2 != 0 {
        2u8
    } else if provided & 1 != 0 && mode == EncryptionMode::Prefer {
        1
    } else {
        bail!("peer offers no acceptable MSE encryption");
    };
    padding(&mut r, &mut decryptor, &header[12..]).await?;
    let len = decrypt(&mut r, &mut decryptor, 2).await?;
    let len = usize::from(u16::from_be_bytes(len.try_into().unwrap()));
    let initial = decrypt(&mut r, &mut decryptor, len).await?;
    let mut response = [0; 14];
    response[11] = selected;
    encrypt.apply_keystream(&mut response);
    w.write_all(&response).await?;
    w.flush().await?;
    let (r, w) = wrap(
        r,
        w,
        (selected == 2).then_some(decryptor),
        (selected == 2).then_some(encrypt),
        initial,
    );
    Ok((r, w, Some(key)))
}

struct Reader {
    inner: BoxAsyncReadVectored,
    cipher: Option<Cipher>,
    prefix: io::Cursor<Vec<u8>>,
}
impl AsyncRead for Reader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let start = this.prefix.position() as usize;
        let len = (this.prefix.get_ref().len() - start).min(buf.remaining());
        if len > 0 {
            buf.put_slice(&this.prefix.get_ref()[start..start + len]);
            this.prefix.set_position((start + len) as u64);
            return Poll::Ready(Ok(()));
        }
        let start = buf.filled().len();
        std::task::ready!(Pin::new(&mut this.inner).poll_read(cx, buf))?;
        if let Some(c) = &mut this.cipher {
            c.apply_keystream(&mut buf.filled_mut()[start..]);
        }
        Poll::Ready(Ok(()))
    }
}

struct Writer {
    inner: BoxAsyncWrite,
    cipher: Cipher,
    keystream: Vec<u8>,
}
impl AsyncWrite for Writer {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let mut bytes = buf[..buf.len().min(16 * 1024)].to_vec();
        // Retain unconsumed keystream across Pending and short writes. A
        // cancelled write can retry with completely different input safely.
        let ready = this.keystream.len();
        if ready < bytes.len() {
            this.keystream.resize(bytes.len(), 0);
            this.cipher.apply_keystream(&mut this.keystream[ready..]);
        }
        for (byte, key) in bytes.iter_mut().zip(&this.keystream) {
            *byte ^= key;
        }
        let n = std::task::ready!(Pin::new(&mut this.inner).poll_write(cx, &bytes))?;
        this.keystream.drain(..n);
        Poll::Ready(Ok(n))
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

fn wrap(
    r: BoxAsyncReadVectored,
    w: BoxAsyncWrite,
    decryptor: Option<Cipher>,
    encrypt: Option<Cipher>,
    initial: Vec<u8>,
) -> Transport {
    let r = Box::new(
        Reader {
            inner: r,
            cipher: decryptor,
            prefix: io::Cursor::new(initial),
        }
        .into_vectored_compat(),
    );
    let w: BoxAsyncWrite = match encrypt {
        Some(cipher) => Box::new(Writer {
            inner: w,
            cipher,
            keystream: Vec::new(),
        }),
        None => w,
    };
    (r, w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    const KEY: Id20 = librqbit_core::hash_id::Id([0x42; 20]);

    fn split(stream: tokio::io::DuplexStream) -> Transport {
        let (r, w) = tokio::io::split(stream);
        (Box::new(r.into_vectored_compat()), Box::new(w))
    }

    #[test]
    fn dh_agrees_and_rejects_degenerate_keys() {
        let a = Exchange::new().unwrap();
        let b = Exchange::new().unwrap();
        assert_eq!(
            a.shared(&b.public()).unwrap(),
            b.shared(&a.public()).unwrap()
        );
        for n in [
            BigUint::from(0u8),
            BigUint::from(1u8),
            &a.prime - BigUint::from(1u8),
            a.prime.clone(),
        ] {
            assert!(a.shared(&fixed_key(&n)).is_err());
        }
        let mut one = [0; 96];
        one[95] = 1;
        assert_eq!(fixed_key(&BigUint::from(1u8)), one);
    }

    #[tokio::test]
    async fn rc4_payload_roundtrip_with_fragmentation_and_short_writes() {
        for mode in [EncryptionMode::Prefer, EncryptionMode::Require] {
            tokio::time::timeout(Duration::from_secs(5), async {
                let (a, b) = tokio::io::duplex(2048);
                let (ar, aw) = split(a);
                let (br, bw) = split(b);
                let (a, b) = tokio::join!(initiate(ar, aw, KEY), accept(br, bw, mode, &[KEY]));
                let (mut ar, mut aw) = a.unwrap();
                let (mut br, mut bw, key) = b.unwrap();
                assert_eq!(key, Some(KEY));
                let payload: Vec<_> = (0..100_000).map(|i| (i % 251) as u8).collect();
                let send = async {
                    aw.write_all(&payload).await.unwrap();
                    let mut reply = vec![0; payload.len()];
                    ar.read_exact(&mut reply).await.unwrap();
                    assert_eq!(reply, payload);
                };
                let receive = async {
                    let mut received = vec![0; payload.len()];
                    br.read_exact(&mut received).await.unwrap();
                    assert_eq!(received, payload);
                    bw.write_all(&received).await.unwrap();
                };
                tokio::join!(send, receive);
            })
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn plain_connections_preserve_header_and_require_rejects_them() {
        for mode in [
            EncryptionMode::Disabled,
            EncryptionMode::Prefer,
            EncryptionMode::Require,
        ] {
            let (mut a, b) = tokio::io::duplex(128);
            a.write_all(PLAIN_HEADER).await.unwrap();
            let (r, w) = split(b);
            let accepted = accept(r, w, mode, &[KEY]).await;
            if mode == EncryptionMode::Require {
                assert!(accepted.is_err());
            } else {
                let (mut r, _, key) = accepted.unwrap();
                let mut header = [0; 20];
                r.read_exact(&mut header).await.unwrap();
                assert_eq!(&header, PLAIN_HEADER);
                assert_eq!(key, None);
            }
        }
    }

    #[tokio::test]
    async fn disabled_rejects_mse_and_unknown_torrents_are_rejected() {
        for (mode, key) in [
            (EncryptionMode::Disabled, KEY),
            (EncryptionMode::Require, Id20::new([1; 20])),
        ] {
            let (a, b) = tokio::io::duplex(2048);
            let (ar, aw) = split(a);
            let (br, bw) = split(b);
            let keys = [key];
            let (a, b) = tokio::time::timeout(Duration::from_secs(5), async {
                tokio::join!(initiate(ar, aw, KEY), accept(br, bw, mode, &keys))
            })
            .await
            .unwrap();
            assert!(a.is_err());
            assert!(b.is_err());
        }
    }

    // A manually framed initiator exercises paths our initiator does not
    // generate: maximum PadA/PadC, IA, and the MSE plaintext method.
    async fn manual_initiator(
        mut r: BoxAsyncReadVectored,
        mut w: BoxAsyncWrite,
        provided: u32,
        pad_len: u16,
        bad_vc: bool,
    ) -> anyhow::Result<Transport> {
        let exchange = Exchange::new()?;
        let mut bytes = exchange.public().to_vec();
        bytes.extend_from_slice(&[0xab; MAX_PAD]);
        w.write_all(&bytes).await?;
        let mut remote = [0; 96];
        r.read_exact(&mut remote).await?;
        let secret = exchange.shared(&remote)?;
        let mut encrypt = cipher(b"keyA", &secret, KEY);
        let mut decryptor = cipher(b"keyB", &secret, KEY);
        let mut bytes = hash(&[b"req1", &secret]).to_vec();
        bytes.extend(
            hash(&[b"req2", &KEY.0])
                .iter()
                .zip(hash(&[b"req3", &secret]))
                .map(|(a, b)| a ^ b),
        );
        let mut frame = vec![0; 8];
        frame[0] = u8::from(bad_vc);
        frame.extend_from_slice(&provided.to_be_bytes());
        frame.extend_from_slice(&pad_len.to_be_bytes());
        frame.resize(frame.len() + usize::from(pad_len), 0);
        frame.extend_from_slice(&20u16.to_be_bytes());
        frame.extend_from_slice(PLAIN_HEADER);
        encrypt.apply_keystream(&mut frame);
        bytes.extend(frame);
        w.write_all(&bytes).await?;
        let mut vc = [0; 8];
        decryptor.apply_keystream(&mut vc);
        sync(&mut r, &vc).await?;
        let response = decrypt(&mut r, &mut decryptor, 6).await?;
        padding(&mut r, &mut decryptor, &response[4..]).await?;
        let rc4 = response[..4] == [0, 0, 0, 2];
        Ok(wrap(
            r,
            w,
            rc4.then_some(decryptor),
            rc4.then_some(encrypt),
            Vec::new(),
        ))
    }

    #[tokio::test]
    async fn responder_preserves_initial_payload_with_maximum_padding() {
        for provided in [1, 2, 3] {
            let (a, b) = tokio::io::duplex(4096);
            let (ar, aw) = split(a);
            let (br, bw) = split(b);
            let (a, b) = tokio::time::timeout(Duration::from_secs(5), async {
                tokio::join!(
                    manual_initiator(ar, aw, provided, 512, false),
                    accept(br, bw, EncryptionMode::Prefer, &[KEY])
                )
            })
            .await
            .unwrap();
            let (mut ar, mut aw) = a.unwrap();
            let (mut br, mut bw, _) = b.unwrap();
            let mut header = [0; 20];
            br.read_exact(&mut header).await.unwrap();
            assert_eq!(&header, PLAIN_HEADER);
            aw.write_all(b"after IA").await.unwrap();
            let mut rest = [0; 8];
            br.read_exact(&mut rest).await.unwrap();
            assert_eq!(&rest, b"after IA");
            bw.write_all(b"response").await.unwrap();
            ar.read_exact(&mut rest).await.unwrap();
            assert_eq!(&rest, b"response");
        }
    }

    #[tokio::test]
    async fn responder_rejects_bad_vc_padding_and_crypto_methods() {
        for (provided, pad, bad_vc, mode) in [
            (2, 513, false, EncryptionMode::Prefer),
            (2, 0, true, EncryptionMode::Prefer),
            (0, 0, false, EncryptionMode::Prefer),
            (4, 0, false, EncryptionMode::Prefer),
            (1, 0, false, EncryptionMode::Require),
        ] {
            let (a, b) = tokio::io::duplex(4096);
            let (ar, aw) = split(a);
            let (br, bw) = split(b);
            let (_, b) = tokio::time::timeout(Duration::from_secs(5), async {
                tokio::join!(
                    manual_initiator(ar, aw, provided, pad, bad_vc),
                    accept(br, bw, mode, &[KEY])
                )
            })
            .await
            .unwrap();
            assert!(b.is_err());
        }
    }

    #[tokio::test]
    async fn initiator_rejects_plaintext_selection_and_oversized_padd() {
        for (selected, pad) in [(0u32, 0u16), (1, 0), (3, 0), (4, 0), (2, 513)] {
            let (a, b) = tokio::io::duplex(4096);
            let (ar, aw) = split(a);
            let (mut br, mut bw) = split(b);
            let responder = async {
                let mut public = [0; 96];
                br.read_exact(&mut public).await.unwrap();
                let exchange = Exchange::new().unwrap();
                let secret = exchange.shared(&public).unwrap();
                // Maximum PadB also exercises initiator synchronization.
                let mut response = exchange.public().to_vec();
                response.extend_from_slice(&[0xab; MAX_PAD]);
                bw.write_all(&response).await.unwrap();
                sync(&mut br, &hash(&[b"req1", &secret])).await.unwrap();
                let mut req2_and_frame = [0; 36];
                br.read_exact(&mut req2_and_frame).await.unwrap();
                let mut encrypted = vec![0; 8];
                encrypted.extend_from_slice(&selected.to_be_bytes());
                encrypted.extend_from_slice(&pad.to_be_bytes());
                cipher(b"keyB", &secret, KEY).apply_keystream(&mut encrypted);
                bw.write_all(&encrypted).await.unwrap();
            };
            let (result, _) = tokio::time::timeout(Duration::from_secs(5), async {
                tokio::join!(initiate(ar, aw, KEY), responder)
            })
            .await
            .unwrap();
            assert!(result.is_err());
        }
    }

    #[tokio::test]
    async fn synchronization_accepts_max_padding_and_rejects_excess() {
        for len in [0, 512, 513] {
            let (mut a, b) = tokio::io::duplex(1024);
            let mut bytes = vec![0xff; len];
            bytes.extend_from_slice(b"marker");
            a.write_all(&bytes).await.unwrap();
            let (mut r, _) = split(b);
            assert_eq!(sync(&mut r, b"marker").await.is_ok(), len <= 512);
        }
    }

    struct ShortWriter {
        bytes: Arc<Mutex<Vec<u8>>>,
        pending: bool,
    }
    impl AsyncWrite for ShortWriter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            if self.pending {
                self.pending = false;
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            let n = bytes.len().min(3);
            self.bytes.lock().unwrap().extend_from_slice(&bytes[..n]);
            Poll::Ready(Ok(n))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn cancelled_pending_write_does_not_consume_keystream() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let secret = [7; 96];
        let mut w = Writer {
            inner: Box::new(ShortWriter {
                bytes: bytes.clone(),
                pending: true,
            }),
            cipher: cipher(b"keyA", &secret, KEY),
            keystream: Vec::new(),
        };
        std::future::poll_fn(|cx| {
            assert!(
                Pin::new(&mut w)
                    .poll_write(cx, b"cancelled long write")
                    .is_pending()
            );
            Poll::Ready(())
        })
        .await;
        w.write_all(b"short").await.unwrap();
        w.write_all(b" then a much longer subsequent write")
            .await
            .unwrap();
        let mut received = bytes.lock().unwrap().clone();
        cipher(b"keyA", &secret, KEY).apply_keystream(&mut received);
        assert_eq!(received, b"short then a much longer subsequent write");
    }
}
