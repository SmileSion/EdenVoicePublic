use anyhow::{Context, Result, anyhow, bail, ensure};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use hkdf::Hkdf;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::{
    avatar,
    config::ClientConfig,
    security::{
        Identity, export_trust_store_for_backup, restore_trust_store_from_backup,
        validate_trust_store_backup,
    },
};

const BACKUP_VERSION: u8 = 1;
const PAYLOAD_VERSION: u8 = 1;
const MAX_BACKUP_BYTES: usize = 4 * 1024 * 1024;
const ARGON2_MEMORY_KIB: u32 = 64 * 1024;
const ARGON2_ITERATIONS: u32 = 3;
const ARGON2_PARALLELISM: u32 = 1;
const BACKUP_AAD: &[u8] = b"EdenVoice/user-data-backup/v1";

#[derive(Serialize, Deserialize)]
struct BackupEnvelope {
    version: u8,
    kdf: String,
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
    salt: String,
    cipher: String,
    nonce: String,
    ciphertext: String,
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
struct BackupPayload {
    version: u8,
    settings: String,
    identity: String,
    trusted_servers: Option<String>,
    avatar: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupImport {
    pub client_id: String,
}

pub fn export_encrypted(password: &str) -> Result<Vec<u8>> {
    validate_password(password)?;
    let mut key = Zeroizing::new([0u8; 32]);
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    derive_key(password, &salt, &mut key)?;

    let config = ClientConfig::load();
    let settings = Zeroizing::new(serde_json::to_vec(&config)?);
    let identity = Identity::load_or_create()?;
    let identity_key = derive_identity_key(&key)?;
    let identity_export = Zeroizing::new(identity.export_encrypted(&identity_key)?);
    let trusted_servers =
        export_trust_store_for_backup()?.map(|bytes| STANDARD_NO_PAD.encode(bytes));
    let avatar = if avatar::is_custom_avatar(&config.avatar) {
        Some(avatar::data_for_reference(&config.avatar).context("无法读取自定义头像")?)
    } else {
        None
    };
    let payload = BackupPayload {
        version: PAYLOAD_VERSION,
        settings: STANDARD_NO_PAD.encode(settings.as_slice()),
        identity: STANDARD_NO_PAD.encode(identity_export.as_slice()),
        trusted_servers,
        avatar,
    };
    let plaintext = Zeroizing::new(serde_json::to_vec(&payload)?);
    encrypt_payload(&plaintext, &key, &salt)
}

pub fn import_encrypted(bytes: &[u8], password: &str) -> Result<BackupImport> {
    validate_password(password)?;
    ensure!(bytes.len() <= MAX_BACKUP_BYTES, "备份文件过大");
    let envelope: BackupEnvelope = serde_json::from_slice(bytes).context("备份文件格式无效")?;
    validate_envelope(&envelope)?;
    let salt: [u8; 16] = decode_fixed(&envelope.salt, "备份盐值")?;
    let nonce: [u8; 24] = decode_fixed(&envelope.nonce, "备份随机数")?;
    let ciphertext = Zeroizing::new(
        STANDARD_NO_PAD
            .decode(&envelope.ciphertext)
            .context("备份密文格式无效")?,
    );
    ensure!(ciphertext.len() <= MAX_BACKUP_BYTES, "备份密文过大");
    let mut key = Zeroizing::new([0u8; 32]);
    derive_key(password, &salt, &mut key)?;
    let cipher = XChaCha20Poly1305::new((&*key).into());
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: ciphertext.as_ref(),
                    aad: BACKUP_AAD,
                },
            )
            .map_err(|_| anyhow!("密码错误，或备份文件已损坏"))?,
    );
    let payload: BackupPayload = serde_json::from_slice(&plaintext).context("备份内容格式无效")?;
    ensure!(payload.version == PAYLOAD_VERSION, "不支持的备份内容版本");

    let settings = decode_limited(&payload.settings, 1024 * 1024, "设置数据")?;
    let config = ClientConfig::decode_backup(&settings)?;
    let identity_bytes = decode_limited(&payload.identity, 64 * 1024, "身份数据")?;
    let identity_key = derive_identity_key(&key)?;
    let identity = Identity::import_encrypted(&identity_bytes, &identity_key)
        .map_err(|_| anyhow!("身份数据校验失败，备份可能已损坏"))?;
    let trust_bytes = payload
        .trusted_servers
        .as_deref()
        .map(|value| decode_limited(value, 1024 * 1024, "服务器信任数据"))
        .transpose()?;
    if let Some(bytes) = trust_bytes.as_deref() {
        validate_trust_store_backup(bytes)?;
    }
    if let Some(data) = payload.avatar.as_deref() {
        let reference = avatar::reference_for_backup_data(data)?;
        ensure!(reference == config.avatar, "备份头像与个人资料不匹配");
    } else {
        ensure!(
            !avatar::is_custom_avatar(&config.avatar),
            "备份缺少自定义头像数据"
        );
    }

    if let Some(data) = payload.avatar.as_deref() {
        avatar::cache_upload(data)?;
    }
    restore_trust_store_from_backup(trust_bytes.as_deref().map(Vec::as_slice))?;
    identity.persist_default()?;
    config.save()?;
    Ok(BackupImport {
        client_id: identity.client_id(),
    })
}

fn encrypt_payload(plaintext: &[u8], key: &[u8; 32], salt: &[u8; 16]) -> Result<Vec<u8>> {
    let mut nonce = [0u8; 24];
    OsRng.fill_bytes(&mut nonce);
    let cipher = XChaCha20Poly1305::new(key.into());
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: BACKUP_AAD,
            },
        )
        .map_err(|_| anyhow!("无法加密备份"))?;
    let envelope = BackupEnvelope {
        version: BACKUP_VERSION,
        kdf: "Argon2id".into(),
        memory_kib: ARGON2_MEMORY_KIB,
        iterations: ARGON2_ITERATIONS,
        parallelism: ARGON2_PARALLELISM,
        salt: STANDARD_NO_PAD.encode(salt),
        cipher: "XChaCha20-Poly1305".into(),
        nonce: STANDARD_NO_PAD.encode(nonce),
        ciphertext: STANDARD_NO_PAD.encode(ciphertext),
    };
    Ok(serde_json::to_vec_pretty(&envelope)?)
}

fn validate_password(password: &str) -> Result<()> {
    ensure!(password.chars().count() >= 10, "密码至少需要 10 个字符");
    ensure!(password.len() <= 1024, "密码过长");
    Ok(())
}

fn validate_envelope(envelope: &BackupEnvelope) -> Result<()> {
    ensure!(envelope.version == BACKUP_VERSION, "不支持的备份版本");
    ensure!(envelope.kdf == "Argon2id", "不支持的密码派生算法");
    ensure!(
        envelope.memory_kib == ARGON2_MEMORY_KIB
            && envelope.iterations == ARGON2_ITERATIONS
            && envelope.parallelism == ARGON2_PARALLELISM,
        "备份使用了不支持的密码派生参数"
    );
    ensure!(
        envelope.cipher == "XChaCha20-Poly1305",
        "不支持的备份加密算法"
    );
    Ok(())
}

fn derive_key(password: &str, salt: &[u8], output: &mut [u8; 32]) -> Result<()> {
    let params = Params::new(
        ARGON2_MEMORY_KIB,
        ARGON2_ITERATIONS,
        ARGON2_PARALLELISM,
        Some(32),
    )
    .map_err(|error| anyhow!("无效的密码派生参数：{error}"))?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, output)
        .map_err(|error| anyhow!("无法派生备份密钥：{error}"))
}

fn derive_identity_key(backup_key: &[u8; 32]) -> Result<Zeroizing<[u8; 32]>> {
    let mut identity_key = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(None, backup_key)
        .expand(
            b"EdenVoice/user-data-backup/v1/identity",
            &mut *identity_key,
        )
        .map_err(|_| anyhow!("无法派生身份备份密钥"))?;
    Ok(identity_key)
}

fn decode_fixed<const N: usize>(value: &str, name: &str) -> Result<[u8; N]> {
    STANDARD_NO_PAD
        .decode(value)
        .with_context(|| format!("{name}格式无效"))?
        .try_into()
        .map_err(|_| anyhow!("{name}长度无效"))
}

fn decode_limited(value: &str, maximum: usize, name: &str) -> Result<Zeroizing<Vec<u8>>> {
    let bytes = Zeroizing::new(
        STANDARD_NO_PAD
            .decode(value)
            .with_context(|| format!("{name}格式无效"))?,
    );
    if bytes.len() > maximum {
        bail!("{name}过大");
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encrypted_fixture(password: &str) -> Vec<u8> {
        let salt = [7u8; 16];
        let mut key = [0u8; 32];
        derive_key(password, &salt, &mut key).unwrap();
        encrypt_payload(br#"{\"nickname\":\"Alice\"}"#, &key, &salt).unwrap()
    }

    #[test]
    fn encrypted_backup_does_not_expose_plaintext() {
        let backup = encrypted_fixture("correct horse battery staple");
        assert!(
            !backup
                .windows(b"Alice".len())
                .any(|window| window == b"Alice")
        );
        let envelope: BackupEnvelope = serde_json::from_slice(&backup).unwrap();
        assert_eq!(envelope.kdf, "Argon2id");
        assert_eq!(envelope.cipher, "XChaCha20-Poly1305");
    }

    #[test]
    fn wrong_password_cannot_authenticate_backup() {
        let backup = encrypted_fixture("correct horse battery staple");
        let envelope: BackupEnvelope = serde_json::from_slice(&backup).unwrap();
        let salt: [u8; 16] = decode_fixed(&envelope.salt, "salt").unwrap();
        let nonce: [u8; 24] = decode_fixed(&envelope.nonce, "nonce").unwrap();
        let ciphertext = STANDARD_NO_PAD.decode(envelope.ciphertext).unwrap();
        let mut wrong_key = [0u8; 32];
        derive_key("incorrect password", &salt, &mut wrong_key).unwrap();
        let cipher = XChaCha20Poly1305::new((&wrong_key).into());
        assert!(
            cipher
                .decrypt(
                    XNonce::from_slice(&nonce),
                    Payload {
                        msg: &ciphertext,
                        aad: BACKUP_AAD,
                    },
                )
                .is_err()
        );
    }
}
