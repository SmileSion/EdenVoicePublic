use std::{
    collections::HashMap,
    fs,
    fs::OpenOptions,
    io::Write,
    net::TcpStream,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    thread,
    time::Duration,
};

use base64::{
    Engine,
    engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD},
};
use chacha20poly1305::{
    ChaCha20Poly1305, KeyInit,
    aead::{Aead, Payload},
};
use directories::ProjectDirs;
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use hkdf::Hkdf;
use rand_core::{OsRng, RngCore};
use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, Error as TlsError, SignatureScheme,
    StreamOwned,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
    version::TLS13,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use x509_parser::parse_x509_certificate;
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

pub const PROTOCOL_VERSION: u16 = 5;
pub const INITIAL_KEY_EPOCH: u16 = 1;
pub const SECURITY_CAPABILITIES: &[&str] = &[
    "identity_ed25519",
    "session_x25519_hkdf_sha256",
    "media_chacha20poly1305",
    "media_replay_window",
];
const TRANSCRIPT_DOMAIN: &[u8] = b"EdenVoice/v0.5/identity-auth\0";
const MEDIA_REKEY_GRACE: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct Identity {
    signing_key: SigningKey,
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
struct IdentityFile {
    version: u8,
    public_key: String,
    private_key: String,
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct IdentityExport {
    pub version: u8,
    pub client_id: String,
    pub public_key: String,
    pub cipher: String,
    pub nonce: String,
    pub encrypted_private_key: String,
}

impl Identity {
    pub fn load_or_create() -> anyhow::Result<Self> {
        let path = security_dir()?.join("client-identity.json");
        Self::load_or_create_at(&path)
    }

    fn load_or_create_at(path: &Path) -> anyhow::Result<Self> {
        match fs::read(path) {
            Ok(bytes) => {
                ensure_private_path(path)?;
                Self::decode(&Zeroizing::new(bytes))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let identity = Self {
                    signing_key: SigningKey::generate(&mut OsRng),
                };
                identity.persist(path)?;
                Ok(identity)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    pub fn client_id(&self) -> String {
        URL_SAFE_NO_PAD.encode(Sha256::digest(self.public_key()))
    }

    pub fn sign(&self, transcript: &[u8]) -> [u8; 64] {
        self.signing_key.sign(transcript).to_bytes()
    }

    /// Creates a portable, versioned identity payload protected by a caller-held
    /// 256-bit recovery key. The recovery key is never serialized into the export.
    pub fn export_encrypted(&self, recovery_key: &[u8; 32]) -> anyhow::Result<Vec<u8>> {
        let client_id = self.client_id();
        let public_key = STANDARD_NO_PAD.encode(self.public_key());
        let aad = identity_export_aad(&client_id, &public_key);
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let private_key = Zeroizing::new(self.signing_key.to_bytes());
        let cipher = ChaCha20Poly1305::new(recovery_key.into());
        let ciphertext = cipher
            .encrypt(
                (&nonce).into(),
                Payload {
                    msg: private_key.as_ref(),
                    aad: &aad,
                },
            )
            .map_err(|_| anyhow::anyhow!("identity export encryption failed"))?;
        let export = IdentityExport {
            version: 1,
            client_id,
            public_key,
            cipher: "ChaCha20-Poly1305".into(),
            nonce: STANDARD_NO_PAD.encode(nonce),
            encrypted_private_key: STANDARD_NO_PAD.encode(ciphertext),
        };
        let bytes = Zeroizing::new(serde_json::to_vec_pretty(&export)?);
        Ok(bytes.to_vec())
    }

    /// Decodes an encrypted export without modifying the current identity file.
    /// Callers can validate the returned ClientID before choosing where to persist it.
    pub fn import_encrypted(bytes: &[u8], recovery_key: &[u8; 32]) -> anyhow::Result<Self> {
        let export: IdentityExport = serde_json::from_slice(bytes)?;
        anyhow::ensure!(export.version == 1, "unsupported identity export version");
        anyhow::ensure!(
            export.cipher == "ChaCha20-Poly1305",
            "unsupported identity export cipher"
        );
        let nonce: [u8; 12] = STANDARD_NO_PAD
            .decode(&export.nonce)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid identity export nonce"))?;
        let ciphertext = Zeroizing::new(STANDARD_NO_PAD.decode(&export.encrypted_private_key)?);
        let aad = identity_export_aad(&export.client_id, &export.public_key);
        let cipher = ChaCha20Poly1305::new(recovery_key.into());
        let private_bytes = Zeroizing::new(
            cipher
                .decrypt(
                    (&nonce).into(),
                    Payload {
                        msg: ciphertext.as_ref(),
                        aad: &aad,
                    },
                )
                .map_err(|_| anyhow::anyhow!("identity export authentication failed"))?,
        );
        anyhow::ensure!(private_bytes.len() == 32, "invalid exported private key");
        let mut private = Zeroizing::new([0u8; 32]);
        private.copy_from_slice(&private_bytes);
        let signing_key = SigningKey::from_bytes(&private);
        let expected_public: [u8; 32] = STANDARD_NO_PAD
            .decode(&export.public_key)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid exported public key"))?;
        anyhow::ensure!(
            signing_key.verifying_key().to_bytes() == expected_public,
            "exported identity key pair does not match"
        );
        let identity = Self { signing_key };
        anyhow::ensure!(
            identity.client_id() == export.client_id,
            "exported ClientID does not match its public key"
        );
        Ok(identity)
    }

    pub(crate) fn persist_default(&self) -> anyhow::Result<()> {
        self.persist(&security_dir()?.join("client-identity.json"))
    }

    fn decode(bytes: &[u8]) -> anyhow::Result<Self> {
        let stored: IdentityFile = serde_json::from_slice(bytes)?;
        anyhow::ensure!(stored.version == 1, "unsupported client identity version");
        let private_bytes = Zeroizing::new(STANDARD_NO_PAD.decode(&stored.private_key)?);
        anyhow::ensure!(private_bytes.len() == 32, "invalid client private key");
        let mut private = Zeroizing::new([0u8; 32]);
        private.copy_from_slice(&private_bytes);
        let public: [u8; 32] = STANDARD_NO_PAD
            .decode(&stored.public_key)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid client public key"))?;
        let signing_key = SigningKey::from_bytes(&private);
        anyhow::ensure!(
            signing_key.verifying_key() == VerifyingKey::from_bytes(&public)?,
            "client identity key pair does not match"
        );
        Ok(Self { signing_key })
    }

    fn persist(&self, path: &Path) -> anyhow::Result<()> {
        let private = Zeroizing::new(self.signing_key.to_bytes());
        let stored = IdentityFile {
            version: 1,
            public_key: STANDARD_NO_PAD.encode(self.public_key()),
            private_key: STANDARD_NO_PAD.encode(private.as_ref()),
        };
        let bytes = Zeroizing::new(serde_json::to_vec_pretty(&stored)?);
        write_private_file_atomic(path, &bytes)
    }
}

fn identity_export_aad(client_id: &str, public_key: &str) -> Vec<u8> {
    let mut aad = b"EdenVoice/v0.5/identity-export\0".to_vec();
    aad.extend_from_slice(client_id.as_bytes());
    aad.push(0);
    aad.extend_from_slice(public_key.as_bytes());
    aad
}

#[derive(Clone)]
pub struct EphemeralKey {
    secret: StaticSecret,
    pub public: [u8; 32],
}

impl EphemeralKey {
    pub fn generate() -> Self {
        let secret = StaticSecret::random_from_rng(OsRng);
        let public = X25519PublicKey::from(&secret).to_bytes();
        Self { secret, public }
    }

    pub fn derive(&self, peer: [u8; 32], transcript: &[u8]) -> anyhow::Result<SessionSecrets> {
        let shared = self.secret.diffie_hellman(&X25519PublicKey::from(peer));
        anyhow::ensure!(shared.was_contributory(), "invalid peer ephemeral key");
        let hkdf = Hkdf::<Sha256>::new(Some(transcript), shared.as_bytes());
        let mut material = [0u8; 76];
        hkdf.expand(b"EdenVoice/v0.5/session-secrets", &mut material)
            .map_err(|_| anyhow::anyhow!("session key derivation failed"))?;
        let mut secrets = SessionSecrets::default();
        secrets
            .client_to_server_key
            .copy_from_slice(&material[..32]);
        secrets
            .server_to_client_key
            .copy_from_slice(&material[32..64]);
        secrets
            .client_nonce_prefix
            .copy_from_slice(&material[64..70]);
        secrets
            .server_nonce_prefix
            .copy_from_slice(&material[70..76]);
        material.zeroize();
        Ok(secrets)
    }
}

#[derive(Clone, Default, Zeroize, ZeroizeOnDrop)]
pub struct SessionSecrets {
    pub client_to_server_key: [u8; 32],
    pub server_to_client_key: [u8; 32],
    pub client_nonce_prefix: [u8; 6],
    pub server_nonce_prefix: [u8; 6],
}

#[derive(Clone)]
pub struct MediaSecurity {
    pub outbound: Arc<Mutex<MediaCipher>>,
    pub inbound: Arc<Mutex<MediaCipher>>,
    p2p_secret: StaticSecret,
    send_key: Arc<RwLock<Arc<MediaSendKey>>>,
}

impl std::fmt::Debug for MediaSecurity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MediaSecurity([REDACTED])")
    }
}

impl MediaSecurity {
    pub fn new(secrets: SessionSecrets, ephemeral: &EphemeralKey) -> Self {
        Self {
            outbound: Arc::new(Mutex::new(MediaCipher::new(
                secrets.client_to_server_key,
                secrets.client_nonce_prefix,
                INITIAL_KEY_EPOCH,
            ))),
            inbound: Arc::new(Mutex::new(MediaCipher::new(
                secrets.server_to_client_key,
                secrets.server_nonce_prefix,
                INITIAL_KEY_EPOCH,
            ))),
            p2p_secret: ephemeral.secret.clone(),
            send_key: Arc::new(RwLock::new(Arc::new(MediaSendKey {
                root_key: secrets.client_to_server_key,
                nonce_prefix: secrets.client_nonce_prefix,
                epoch: INITIAL_KEY_EPOCH,
            }))),
        }
    }

    pub fn pairwise(
        &self,
        peer_public: [u8; 32],
        self_id: u64,
        peer_id: u64,
    ) -> anyhow::Result<PairwiseSecurity> {
        let shared = self
            .p2p_secret
            .diffie_hellman(&X25519PublicKey::from(peer_public));
        anyhow::ensure!(shared.was_contributory(), "invalid peer security key");
        let (low, high) = if self_id < peer_id {
            (self_id, peer_id)
        } else {
            (peer_id, self_id)
        };
        let mut info = b"EdenVoice/v0.5/p2p-session\0".to_vec();
        info.extend(low.to_be_bytes());
        info.extend(high.to_be_bytes());
        let hkdf = Hkdf::<Sha256>::new(None, shared.as_bytes());
        let mut material = [0u8; 76];
        hkdf.expand(&info, &mut material)
            .map_err(|_| anyhow::anyhow!("P2P key derivation failed"))?;
        let (out_key, in_key, out_prefix, in_prefix) = if self_id < peer_id {
            (
                &material[..32],
                &material[32..64],
                &material[64..70],
                &material[70..76],
            )
        } else {
            (
                &material[32..64],
                &material[..32],
                &material[70..76],
                &material[64..70],
            )
        };
        let mut outbound_key = [0u8; 32];
        outbound_key.copy_from_slice(out_key);
        let mut inbound_key = [0u8; 32];
        inbound_key.copy_from_slice(in_key);
        let mut outbound_prefix = [0u8; 6];
        outbound_prefix.copy_from_slice(out_prefix);
        let mut inbound_prefix = [0u8; 6];
        inbound_prefix.copy_from_slice(in_prefix);
        material.zeroize();
        let outbound = MediaCipher::new(outbound_key, outbound_prefix, INITIAL_KEY_EPOCH);
        let inbound = MediaCipher::new(inbound_key, inbound_prefix, INITIAL_KEY_EPOCH);
        outbound_key.zeroize();
        inbound_key.zeroize();
        outbound_prefix.zeroize();
        inbound_prefix.zeroize();
        Ok(PairwiseSecurity {
            outbound,
            inbound,
            wrap_rekeyed: false,
        })
    }

    /// Snapshot publication only holds a short pointer lock. Encryption and
    /// stream-key derivation never hold the control-plane cipher mutex.
    pub(crate) fn send_key(&self) -> anyhow::Result<Arc<MediaSendKey>> {
        self.send_key
            .read()
            .map(|key| Arc::clone(&key))
            .map_err(|_| anyhow::anyhow!("media send key unavailable"))
    }

    pub fn prepare_rekey(&self, salt: &[u8], epoch: u16) -> anyhow::Result<()> {
        for shared in [&self.outbound, &self.inbound] {
            let (base_epoch, root) = {
                let cipher = shared
                    .lock()
                    .map_err(|_| anyhow::anyhow!("media security unavailable"))?;
                (cipher.epoch(), Zeroizing::new(cipher.current.root_key))
            };
            // HKDF runs outside the receive mutex, so incoming media only
            // contends with installing the prepared epoch.
            let pending = derive_media_epoch(&root, salt, base_epoch, epoch)?;
            let mut cipher = shared
                .lock()
                .map_err(|_| anyhow::anyhow!("media security unavailable"))?;
            anyhow::ensure!(
                cipher.epoch() == base_epoch,
                "media epoch changed during preparation"
            );
            cipher.pending = Some(pending);
        }
        Ok(())
    }

    pub fn activate_rekey(&self, epoch: u16) -> anyhow::Result<()> {
        let outbound = Arc::downgrade(&self.outbound);
        let inbound = Arc::downgrade(&self.inbound);
        thread::Builder::new()
            .name("media-rekey-grace".into())
            .spawn(move || {
                thread::sleep(MEDIA_REKEY_GRACE);
                if let Some(outbound) = outbound.upgrade()
                    && let Ok(mut cipher) = outbound.lock()
                {
                    cipher.drop_previous_if_current(epoch);
                }
                if let Some(inbound) = inbound.upgrade()
                    && let Ok(mut cipher) = inbound.lock()
                {
                    cipher.drop_previous_if_current(epoch);
                }
            })
            .map_err(|error| anyhow::anyhow!("media rekey grace timer failed: {error}"))?;
        {
            let mut cipher = self
                .outbound
                .lock()
                .map_err(|_| anyhow::anyhow!("media security unavailable"))?;
            cipher.activate_prepared(epoch)?;
            let key = Arc::new(MediaSendKey {
                root_key: cipher.current.root_key,
                nonce_prefix: cipher.current.nonce_prefix,
                epoch,
            });
            *self
                .send_key
                .write()
                .map_err(|_| anyhow::anyhow!("media send key unavailable"))? = key;
        }
        self.inbound
            .lock()
            .map_err(|_| anyhow::anyhow!("media security unavailable"))?
            .activate_prepared(epoch)
    }
}

pub struct PairwiseSecurity {
    outbound: MediaCipher,
    inbound: MediaCipher,
    wrap_rekeyed: bool,
}

impl PairwiseSecurity {
    pub fn outbound_epoch(&mut self, sequence: u32) -> anyhow::Result<u16> {
        if sequence >= u32::MAX - 4096 {
            self.wrap_rekeyed = true;
        } else if sequence < 4096 && self.wrap_rekeyed {
            let epoch = self
                .outbound
                .epoch()
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("P2P media epoch exhausted"))?;
            let salt = p2p_epoch_salt(epoch);
            self.outbound.prepare_rekey(&salt, epoch)?;
            self.outbound.activate_prepared(epoch)?;
            self.wrap_rekeyed = false;
        }
        Ok(self.outbound.epoch())
    }

    pub fn seal(
        &mut self,
        aad: &[u8],
        stream_id: u64,
        sequence: u32,
        plaintext: &[u8],
    ) -> anyhow::Result<Vec<u8>> {
        self.outbound.seal(aad, stream_id, sequence, plaintext)
    }

    pub fn open(
        &mut self,
        aad: &[u8],
        stream_id: u64,
        epoch: u16,
        sequence: u32,
        ciphertext: &[u8],
    ) -> anyhow::Result<Vec<u8>> {
        let advance = epoch == self.inbound.epoch().wrapping_add(1) && epoch != 0;
        if advance {
            self.inbound.prepare_rekey(&p2p_epoch_salt(epoch), epoch)?;
        }
        let plaintext = self
            .inbound
            .open(aad, stream_id, epoch, sequence, ciphertext)?;
        if advance {
            self.inbound.activate_prepared(epoch)?;
        }
        Ok(plaintext)
    }
}

fn p2p_epoch_salt(epoch: u16) -> Vec<u8> {
    let mut salt = b"EdenVoice/v0.5/p2p-wrap\0".to_vec();
    salt.extend(epoch.to_be_bytes());
    salt
}

pub struct MediaCipher {
    current: EpochCipher,
    previous: Option<EpochCipher>,
    pending: Option<EpochCipher>,
}

struct EpochCipher {
    epoch: u16,
    root_key: [u8; 32],
    nonce_prefix: [u8; 6],
    streams: HashMap<u64, ChaCha20Poly1305>,
    replay: HashMap<u64, ReplayWindow>,
}

#[derive(Default)]
struct ReplayWindow {
    initialized: bool,
    highest: u32,
    bitmap: u64,
}

impl ReplayWindow {
    fn seen(&self, sequence: u32) -> bool {
        if !self.initialized || sequence > self.highest {
            return false;
        }
        let delta = self.highest - sequence;
        delta >= 64 || self.bitmap & (1u64 << delta) != 0
    }
    fn mark(&mut self, sequence: u32) {
        if !self.initialized {
            self.initialized = true;
            self.highest = sequence;
            self.bitmap = 1;
        } else if sequence > self.highest {
            let delta = sequence - self.highest;
            self.bitmap = if delta >= 64 {
                1
            } else {
                self.bitmap << delta | 1
            };
            self.highest = sequence;
        } else {
            self.bitmap |= 1u64 << (self.highest - sequence);
        }
    }
}

impl MediaCipher {
    pub fn new(root_key: [u8; 32], nonce_prefix: [u8; 6], epoch: u16) -> Self {
        Self {
            current: EpochCipher::new(root_key, nonce_prefix, epoch),
            previous: None,
            pending: None,
        }
    }

    pub fn epoch(&self) -> u16 {
        self.current.epoch
    }

    pub fn seal(
        &mut self,
        aad: &[u8],
        stream_id: u64,
        sequence: u32,
        plaintext: &[u8],
    ) -> anyhow::Result<Vec<u8>> {
        let nonce = media_nonce(self.current.nonce_prefix, self.current.epoch, sequence);
        let cipher = self.current.stream_cipher(stream_id)?;
        cipher
            .encrypt(
                (&nonce).into(),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| anyhow::anyhow!("media encryption failed"))
    }

    pub fn open(
        &mut self,
        aad: &[u8],
        stream_id: u64,
        epoch: u16,
        sequence: u32,
        ciphertext: &[u8],
    ) -> anyhow::Result<Vec<u8>> {
        let selected = if self.current.epoch == epoch {
            &mut self.current
        } else if self
            .previous
            .as_ref()
            .is_some_and(|value| value.epoch == epoch)
        {
            self.previous.as_mut().expect("previous epoch checked")
        } else if self
            .pending
            .as_ref()
            .is_some_and(|value| value.epoch == epoch)
        {
            self.pending.as_mut().expect("pending epoch checked")
        } else {
            anyhow::bail!("invalid media key epoch")
        };
        let window = selected.replay.entry(stream_id).or_default();
        anyhow::ensure!(!window.seen(sequence), "replayed or stale media packet");
        let nonce = media_nonce(selected.nonce_prefix, epoch, sequence);
        let plaintext = selected
            .stream_cipher(stream_id)?
            .decrypt(
                (&nonce).into(),
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| anyhow::anyhow!("media authentication failed"))?;
        selected.replay.entry(stream_id).or_default().mark(sequence);
        Ok(plaintext)
    }

    pub fn prepare_rekey(&mut self, salt: &[u8], epoch: u16) -> anyhow::Result<()> {
        self.pending = Some(derive_media_epoch(
            &self.current.root_key,
            salt,
            self.current.epoch,
            epoch,
        )?);
        Ok(())
    }

    pub fn activate_prepared(&mut self, epoch: u16) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.pending
                .as_ref()
                .is_some_and(|value| value.epoch == epoch),
            "media rekey was not prepared"
        );
        let pending = self.pending.take().expect("pending epoch checked");
        self.previous = Some(std::mem::replace(&mut self.current, pending));
        Ok(())
    }

    fn drop_previous_if_current(&mut self, current_epoch: u16) {
        if self.current.epoch == current_epoch {
            self.previous = None;
        }
    }
}

// A send-only immutable key has no sequence allocator or replay state. All
// adaptive senders continue sharing the session's atomic sequence allocator.
#[derive(Zeroize, ZeroizeOnDrop)]
pub(crate) struct MediaSendKey {
    root_key: [u8; 32],
    nonce_prefix: [u8; 6],
    epoch: u16,
}

impl MediaSendKey {
    pub(crate) fn epoch(&self) -> u16 {
        self.epoch
    }

    pub(crate) fn seal(
        &self,
        aad: &[u8],
        stream_id: u64,
        sequence: u32,
        plaintext: &[u8],
    ) -> anyhow::Result<Vec<u8>> {
        let cipher = derive_stream_cipher(&self.root_key, stream_id)?;
        let nonce = media_nonce(self.nonce_prefix, self.epoch, sequence);
        cipher
            .encrypt(
                (&nonce).into(),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| anyhow::anyhow!("media encryption failed"))
    }
}

fn derive_stream_cipher(root_key: &[u8; 32], stream_id: u64) -> anyhow::Result<ChaCha20Poly1305> {
    const DOMAIN: &[u8] = b"EdenVoice/v0.5/media-stream\0";
    let mut info = [0u8; DOMAIN.len() + 8];
    info[..DOMAIN.len()].copy_from_slice(DOMAIN);
    info[DOMAIN.len()..].copy_from_slice(&stream_id.to_be_bytes());
    let mut key = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(None, root_key)
        .expand(&info, &mut *key)
        .map_err(|_| anyhow::anyhow!("media stream key derivation failed"))?;
    Ok(ChaCha20Poly1305::new((&*key).into()))
}

fn derive_media_epoch(
    root_key: &[u8; 32],
    salt: &[u8],
    current_epoch: u16,
    epoch: u16,
) -> anyhow::Result<EpochCipher> {
    anyhow::ensure!(
        epoch != 0 && epoch != current_epoch,
        "invalid pending media epoch"
    );
    let hkdf = Hkdf::<Sha256>::new(Some(salt), root_key);
    let mut material = [0u8; 38];
    hkdf.expand(b"EdenVoice/v0.5/media-rekey", &mut material)
        .map_err(|_| anyhow::anyhow!("media rekey derivation failed"))?;
    let mut key = [0u8; 32];
    key.copy_from_slice(&material[..32]);
    let mut prefix = [0u8; 6];
    prefix.copy_from_slice(&material[32..]);
    material.zeroize();
    let pending = EpochCipher::new(key, prefix, epoch);
    key.zeroize();
    prefix.zeroize();
    Ok(pending)
}

impl EpochCipher {
    fn new(mut root_key: [u8; 32], mut nonce_prefix: [u8; 6], epoch: u16) -> Self {
        let cipher = Self {
            epoch,
            root_key,
            nonce_prefix,
            streams: HashMap::new(),
            replay: HashMap::new(),
        };
        root_key.zeroize();
        nonce_prefix.zeroize();
        cipher
    }
    fn stream_cipher(&mut self, stream_id: u64) -> anyhow::Result<&ChaCha20Poly1305> {
        if !self.streams.contains_key(&stream_id) {
            self.streams
                .insert(stream_id, derive_stream_cipher(&self.root_key, stream_id)?);
        }
        Ok(self
            .streams
            .get(&stream_id)
            .expect("stream cipher inserted"))
    }

    fn zeroize_secrets(&mut self) {
        self.root_key.zeroize();
        self.nonce_prefix.zeroize();
        self.streams.clear();
    }
}

impl Drop for EpochCipher {
    fn drop(&mut self) {
        self.zeroize_secrets();
    }
}

fn media_nonce(prefix: [u8; 6], epoch: u16, sequence: u32) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[..6].copy_from_slice(&prefix);
    nonce[6..8].copy_from_slice(&epoch.to_be_bytes());
    nonce[8..].copy_from_slice(&sequence.to_be_bytes());
    nonce
}

pub fn auth_transcript(
    challenge: &[u8],
    server_identity: &[u8],
    client_ephemeral: &[u8],
    server_ephemeral: &[u8],
) -> Vec<u8> {
    let mut hash = Sha256::new();
    hash.update(TRANSCRIPT_DOMAIN);
    hash.update(PROTOCOL_VERSION.to_be_bytes());
    for field in [
        challenge,
        server_identity,
        client_ephemeral,
        server_ephemeral,
    ] {
        hash.update((field.len() as u32).to_be_bytes());
        hash.update(field);
    }
    hash.finalize().to_vec()
}

#[derive(Debug)]
struct ApplicationVerifiedCertificate;

impl ServerCertVerifier for ApplicationVerifiedCertificate {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(
            message,
            certificate,
            signature,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        if signature.scheme == SignatureScheme::ED25519 {
            rustls::crypto::verify_tls13_signature(
                message,
                certificate,
                signature,
                &rustls::crypto::ring::default_provider().signature_verification_algorithms,
            )
        } else {
            Err(TlsError::General(
                "server certificate must use Ed25519".into(),
            ))
        }
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub type SecureStream = StreamOwned<ClientConnection, TcpStream>;

pub fn tls_connect(stream: TcpStream, host: &str) -> anyhow::Result<SecureStream> {
    let provider = rustls::crypto::ring::default_provider();
    let config = ClientConfig::builder_with_provider(provider.into())
        .with_protocol_versions(&[&TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(ApplicationVerifiedCertificate))
        .with_no_client_auth();
    let name = ServerName::try_from(host.to_owned())
        .or_else(|_| "edenvoice.invalid".to_owned().try_into())?;
    let connection = ClientConnection::new(Arc::new(config), name)?;
    Ok(StreamOwned::new(connection, stream))
}

pub fn peer_identity_public_key(stream: &SecureStream) -> anyhow::Result<Vec<u8>> {
    let certificate = stream
        .conn
        .peer_certificates()
        .and_then(|chain| chain.first())
        .ok_or_else(|| anyhow::anyhow!("server did not provide a TLS certificate"))?;
    let (_, certificate) = parse_x509_certificate(certificate.as_ref())
        .map_err(|_| anyhow::anyhow!("invalid server TLS certificate"))?;
    Ok(certificate
        .tbs_certificate
        .subject_pki
        .subject_public_key
        .data
        .to_vec())
}

#[derive(Default, Serialize, Deserialize)]
struct TrustStore {
    servers: std::collections::BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrustStatus {
    Trusted,
    Unknown,
}

pub fn server_trust_status(endpoint: &str, fingerprint: &str) -> anyhow::Result<TrustStatus> {
    let path = security_dir()?.join("known-servers.json");
    server_trust_status_at(&path, endpoint, fingerprint)
}

fn server_trust_status_at(
    path: &Path,
    endpoint: &str,
    fingerprint: &str,
) -> anyhow::Result<TrustStatus> {
    let store = load_trust_store(path)?;
    if let Some(known) = store.servers.get(endpoint) {
        anyhow::ensure!(known == fingerprint, "Server Identity Changed");
        return Ok(TrustStatus::Trusted);
    }
    Ok(TrustStatus::Unknown)
}

pub fn record_server_trust(endpoint: &str, fingerprint: &str) -> anyhow::Result<()> {
    let path = security_dir()?.join("known-servers.json");
    record_server_trust_at(&path, endpoint, fingerprint)
}

fn record_server_trust_at(path: &Path, endpoint: &str, fingerprint: &str) -> anyhow::Result<()> {
    let mut store = load_trust_store(path)?;
    store
        .servers
        .insert(endpoint.to_owned(), fingerprint.to_owned());
    write_private_file_atomic(path, &serde_json::to_vec_pretty(&store)?)
}

fn load_trust_store(path: &Path) -> anyhow::Result<TrustStore> {
    match fs::read(path) {
        Ok(bytes) => {
            ensure_private_path(path)?;
            serde_json::from_slice(&bytes)
                .map_err(|error| anyhow::anyhow!("invalid server trust store: {error}"))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(TrustStore::default()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn export_trust_store_for_backup() -> anyhow::Result<Option<Vec<u8>>> {
    let path = security_dir()?.join("known-servers.json");
    match fs::read(&path) {
        Ok(bytes) => {
            ensure_private_path(&path)?;
            validate_trust_store_backup(&bytes)?;
            Ok(Some(bytes))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn validate_trust_store_backup(bytes: &[u8]) -> anyhow::Result<()> {
    anyhow::ensure!(
        bytes.len() <= 1024 * 1024,
        "server trust store is too large"
    );
    let store: TrustStore = serde_json::from_slice(bytes)
        .map_err(|error| anyhow::anyhow!("invalid server trust store: {error}"))?;
    anyhow::ensure!(store.servers.len() <= 1024, "too many trusted servers");
    anyhow::ensure!(
        store.servers.iter().all(|(endpoint, fingerprint)| {
            !endpoint.is_empty()
                && endpoint.len() <= 2048
                && !fingerprint.is_empty()
                && fingerprint.len() <= 256
        }),
        "invalid trusted server entry"
    );
    Ok(())
}

pub(crate) fn restore_trust_store_from_backup(bytes: Option<&[u8]>) -> anyhow::Result<()> {
    let path = security_dir()?.join("known-servers.json");
    if let Some(bytes) = bytes {
        validate_trust_store_backup(bytes)?;
        write_private_file_atomic(&path, bytes)
    } else {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

fn ensure_private_path(path: &Path) -> anyhow::Result<()> {
    #[cfg(not(unix))]
    let _ = path;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(parent) = path.parent() {
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn write_private_file_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
    }
    #[cfg(not(windows))]
    let target = path.with_extension("tmp");
    #[cfg(windows)]
    let target = path.to_path_buf();
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&target)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    #[cfg(not(windows))]
    fs::rename(target, path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn security_dir() -> anyhow::Result<PathBuf> {
    let path = ProjectDirs::from("dev", "EdenVoice", "EdenVoice")
        .ok_or_else(|| anyhow::anyhow!("config directory is unavailable"))?
        .config_dir()
        .join("security");
    fs::create_dir_all(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_identity_persists_with_private_permissions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("security/client-identity.json");
        let first = Identity::load_or_create_at(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let second = Identity::load_or_create_at(&path).unwrap();
        assert_eq!(first.client_id(), second.client_id());
        assert_eq!(first.public_key(), second.public_key());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn encrypted_identity_export_round_trips_and_rejects_wrong_key() {
        let identity = Identity {
            signing_key: SigningKey::generate(&mut OsRng),
        };
        let recovery_key = [7u8; 32];
        let exported = identity.export_encrypted(&recovery_key).unwrap();
        let restored = Identity::import_encrypted(&exported, &recovery_key).unwrap();
        assert_eq!(restored.client_id(), identity.client_id());
        assert_eq!(restored.public_key(), identity.public_key());
        assert!(Identity::import_encrypted(&exported, &[8u8; 32]).is_err());

        let mut tampered: serde_json::Value = serde_json::from_slice(&exported).unwrap();
        tampered["client_id"] = serde_json::Value::String("forged".into());
        assert!(
            Identity::import_encrypted(&serde_json::to_vec(&tampered).unwrap(), &recovery_key)
                .is_err()
        );
    }

    #[test]
    fn server_trust_is_pinned_and_corruption_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("security/known-servers.json");
        assert_eq!(
            server_trust_status_at(&path, "voice.example:7001", "fingerprint-a").unwrap(),
            TrustStatus::Unknown
        );
        record_server_trust_at(&path, "voice.example:7001", "fingerprint-a").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(
            server_trust_status_at(&path, "voice.example:7001", "fingerprint-a").unwrap(),
            TrustStatus::Trusted
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        assert!(
            server_trust_status_at(&path, "voice.example:7001", "fingerprint-b")
                .unwrap_err()
                .to_string()
                .contains("Server Identity Changed")
        );

        fs::write(&path, b"not-json").unwrap();
        assert!(server_trust_status_at(&path, "voice.example:7001", "fingerprint-a").is_err());
        assert!(record_server_trust_at(&path, "other.example:7001", "fingerprint-c").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"not-json");
    }

    #[test]
    fn transcript_is_context_bound() {
        assert_ne!(
            auth_transcript(b"a", &[1; 32], &[2; 32], &[3; 32]),
            auth_transcript(b"b", &[1; 32], &[2; 32], &[3; 32])
        );
    }

    #[test]
    fn x25519_peers_derive_matching_fresh_secrets() {
        let client = EphemeralKey::generate();
        let server = EphemeralKey::generate();
        let transcript = b"context";
        let a = client.derive(server.public, transcript).unwrap();
        let b = server.derive(client.public, transcript).unwrap();
        assert_eq!(a.client_to_server_key, b.client_to_server_key);
        let reconnected = server.derive(client.public, b"new context").unwrap();
        assert_ne!(a.client_to_server_key, reconnected.client_to_server_key);
    }

    #[test]
    fn media_cipher_rejects_tamper_replay_and_accepts_reorder() {
        let key = [7u8; 32];
        let prefix = [3u8; 6];
        let mut sender = MediaCipher::new(key, prefix, 1);
        let mut receiver = MediaCipher::new(key, prefix, 1);
        let aad = b"header";
        let packet10 = sender.seal(aad, 42, 10, b"opus").unwrap();
        let packet12 = sender.seal(aad, 42, 12, b"later").unwrap();
        let packet11 = sender.seal(aad, 42, 11, b"reordered").unwrap();
        assert_eq!(receiver.open(aad, 42, 1, 10, &packet10).unwrap(), b"opus");
        assert!(receiver.open(aad, 42, 1, 10, &packet10).is_err());
        assert!(receiver.open(aad, 42, 1, 12, &packet12).is_ok());
        assert!(receiver.open(aad, 42, 1, 11, &packet11).is_ok());
        let mut fresh = MediaCipher::new(key, prefix, 1);
        let mut tampered = packet10.clone();
        tampered[0] ^= 1;
        assert!(fresh.open(aad, 42, 1, 10, &tampered).is_err());
        assert!(
            fresh
                .open(b"tampered header", 42, 1, 10, &packet10)
                .is_err()
        );
        assert!(fresh.open(aad, 42, 2, 10, &packet10).is_err());
    }

    #[test]
    fn prepared_media_epoch_accepts_in_flight_packets_then_activates() {
        let key = [9u8; 32];
        let prefix = [4u8; 6];
        let salt = b"coordinated rekey";
        let mut sender = MediaCipher::new(key, prefix, 1);
        let mut receiver = MediaCipher::new(key, prefix, 1);
        sender.prepare_rekey(salt, 2).unwrap();
        receiver.prepare_rekey(salt, 2).unwrap();
        sender.activate_prepared(2).unwrap();

        let ciphertext = sender.seal(b"header", 7, 1, b"future epoch").unwrap();
        assert_eq!(
            receiver.open(b"header", 7, 2, 1, &ciphertext).unwrap(),
            b"future epoch"
        );
        receiver.activate_prepared(2).unwrap();
        assert_eq!(receiver.epoch(), 2);
    }

    #[test]
    fn previous_media_epoch_is_rejected_after_the_grace_cleanup() {
        let key = [9u8; 32];
        let prefix = [4u8; 6];
        let salt = b"coordinated rekey";
        let mut sender = MediaCipher::new(key, prefix, 1);
        let mut receiver = MediaCipher::new(key, prefix, 1);
        let accepted_during_grace = sender.seal(b"header", 7, 1, b"old one").unwrap();
        let rejected_after_grace = sender.seal(b"header", 7, 2, b"old two").unwrap();
        sender.prepare_rekey(salt, 2).unwrap();
        receiver.prepare_rekey(salt, 2).unwrap();
        sender.activate_prepared(2).unwrap();
        receiver.activate_prepared(2).unwrap();

        assert_eq!(
            receiver
                .open(b"header", 7, 1, 1, &accepted_during_grace)
                .unwrap(),
            b"old one"
        );
        receiver.drop_previous_if_current(2);
        assert!(
            receiver
                .open(b"header", 7, 1, 2, &rejected_after_grace)
                .is_err()
        );
    }

    #[test]
    fn session_and_epoch_secret_buffers_are_explicitly_zeroized() {
        let mut identity_file = IdentityFile {
            version: 1,
            public_key: "public".into(),
            private_key: "private".into(),
        };
        identity_file.zeroize();
        assert_eq!(identity_file.version, 0);
        assert!(identity_file.public_key.is_empty());
        assert!(identity_file.private_key.is_empty());

        let mut secrets = SessionSecrets {
            client_to_server_key: [1; 32],
            server_to_client_key: [2; 32],
            client_nonce_prefix: [3; 6],
            server_nonce_prefix: [4; 6],
        };
        secrets.zeroize();
        assert_eq!(secrets.client_to_server_key, [0; 32]);
        assert_eq!(secrets.server_to_client_key, [0; 32]);
        assert_eq!(secrets.client_nonce_prefix, [0; 6]);
        assert_eq!(secrets.server_nonce_prefix, [0; 6]);

        let mut epoch = EpochCipher::new([5; 32], [6; 6], 1);
        epoch.stream_cipher(7).unwrap();
        epoch.zeroize_secrets();
        assert_eq!(epoch.root_key, [0; 32]);
        assert_eq!(epoch.nonce_prefix, [0; 6]);
        assert!(epoch.streams.is_empty());
    }

    #[test]
    fn send_key_snapshots_follow_rekey_and_preserve_authentication_and_replay() {
        let media = MediaSecurity::new(SessionSecrets::default(), &EphemeralKey::generate());
        let mut receiver = MediaCipher::new([0; 32], [0; 6], 1);
        let old_key = media.send_key().unwrap();
        let old_packet = old_key.seal(b"old header", 77, 1, b"old voice").unwrap();
        media.prepare_rekey(b"snapshot salt", 2).unwrap();
        receiver.prepare_rekey(b"snapshot salt", 2).unwrap();
        assert_eq!(media.send_key().unwrap().epoch(), 1);
        media.activate_rekey(2).unwrap();
        receiver.activate_prepared(2).unwrap();
        let new_key = media.send_key().unwrap();
        assert_eq!(new_key.epoch(), 2);
        let packet = new_key.seal(b"header", 77, 2, b"voice").unwrap();
        assert!(receiver.open(b"tampered", 77, 2, 2, &packet).is_err());
        assert_eq!(
            receiver.open(b"header", 77, 2, 2, &packet).unwrap(),
            b"voice"
        );
        assert!(receiver.open(b"header", 77, 2, 2, &packet).is_err());
        assert_eq!(
            receiver.open(b"old header", 77, 1, 1, &old_packet).unwrap(),
            b"old voice"
        );
        receiver.drop_previous_if_current(2);
        let late = old_key.seal(b"late", 77, 3, b"expired").unwrap();
        assert!(receiver.open(b"late", 77, 1, 3, &late).is_err());
    }

    #[test]
    fn rekey_grace_timer_does_not_retain_a_disconnected_session() {
        let ephemeral = EphemeralKey::generate();
        let media = MediaSecurity::new(SessionSecrets::default(), &ephemeral);
        let outbound = Arc::downgrade(&media.outbound);
        let inbound = Arc::downgrade(&media.inbound);
        media.prepare_rekey(b"timer ownership", 2).unwrap();
        media.activate_rekey(2).unwrap();

        drop(media);

        assert!(outbound.upgrade().is_none());
        assert!(inbound.upgrade().is_none());
    }

    #[test]
    fn pairwise_contexts_are_directionally_compatible_and_separate() {
        let a_key = EphemeralKey::generate();
        let b_key = EphemeralKey::generate();
        let a_media = MediaSecurity::new(SessionSecrets::default(), &a_key);
        let b_media = MediaSecurity::new(SessionSecrets::default(), &b_key);
        let mut a = a_media.pairwise(b_key.public, 10, 20).unwrap();
        let mut b = b_media.pairwise(a_key.public, 20, 10).unwrap();
        let epoch = a.outbound_epoch(1).unwrap();
        let ciphertext = a.seal(b"p2p header", 10, 1, b"pairwise opus").unwrap();
        assert_eq!(
            b.open(b"p2p header", 10, epoch, 1, &ciphertext).unwrap(),
            b"pairwise opus"
        );
        assert!(b.open(b"p2p header", 10, epoch, 1, &ciphertext).is_err());
    }

    #[test]
    fn pairwise_rekeys_before_sequence_wrap() {
        let a_key = EphemeralKey::generate();
        let b_key = EphemeralKey::generate();
        let a_media = MediaSecurity::new(SessionSecrets::default(), &a_key);
        let b_media = MediaSecurity::new(SessionSecrets::default(), &b_key);
        let mut a = a_media.pairwise(b_key.public, 10, 20).unwrap();
        let mut b = b_media.pairwise(a_key.public, 20, 10).unwrap();
        let high = u32::MAX - 1;
        let epoch = a.outbound_epoch(high).unwrap();
        assert_eq!(epoch, INITIAL_KEY_EPOCH);
        let ciphertext = a.seal(b"p2p", 10, high, b"before wrap").unwrap();
        assert_eq!(
            b.open(b"p2p", 10, epoch, high, &ciphertext).unwrap(),
            b"before wrap"
        );

        let wrapped_epoch = a.outbound_epoch(0).unwrap();
        assert_eq!(wrapped_epoch, epoch + 1);
        let ciphertext = a.seal(b"p2p", 10, 0, b"after wrap").unwrap();
        assert_eq!(
            b.open(b"p2p", 10, wrapped_epoch, 0, &ciphertext).unwrap(),
            b"after wrap"
        );
    }
}
