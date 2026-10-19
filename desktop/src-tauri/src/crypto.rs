//! Sync cryptography: thin wrappers over reviewed RustCrypto / dalek crates.
//! No primitive is implemented here. AEAD = XChaCha20-Poly1305 (`chacha20poly1305`),
//! KDF = HKDF-SHA256 (`hkdf`), MAC = HMAC-SHA256 (`hmac`), key agreement = X25519
//! (`x25519-dalek`), randomness = OS CSPRNG (`getrandom`), errors propagated.
//!
//! Wire formats (all fail closed on any other version byte or length):
//! - AEAD blob v2: `0x02 || nonce[24] || ciphertext || tag[16]`; the AEAD's
//!   associated data is `0x02 || aad`, so the version byte is authenticated too.
//! - Wrap blob v2: `0x12 || eph_pub[32] || sender_pub[32] || AEAD blob v2`, keyed by
//!   HKDF-SHA256(DH(eph, recipient) || DH(sender, recipient)), info binding all three
//!   public keys (HPKE-Auth-style sender authentication; see BUILD-BLOCKERS.md for
//!   its key-compromise-impersonation limit).
//!
//! The v1 format (ChaCha20-Poly1305, 12-byte nonce, handwritten) is NOT readable.
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, Zeroizing};

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;
pub const TAG_LEN: usize = 16;
pub const AEAD_V2: u8 = 0x02;
pub const WRAP_V2: u8 = 0x12;
const AEAD_HEADER: usize = 1 + NONCE_LEN;
const WRAP_HEADER: usize = 1 + 32 + 32;

#[derive(Debug, PartialEq, Eq)]
pub enum CryptoError {
    /// Tag mismatch: wrong key, tampered ciphertext/nonce, or mismatched AAD.
    Auth,
    Malformed,
    /// Unknown/legacy format version; never decrypted.
    UnsupportedVersion(u8),
    /// The OS CSPRNG failed; nothing was encrypted.
    Rng,
    /// X25519 produced a non-contributory (low-order) shared secret.
    WeakKey,
}

/// OS CSPRNG. Errors propagate; there is no fallback source.
pub fn fill_random(buf: &mut [u8]) -> Result<(), CryptoError> {
    getrandom::getrandom(buf).map_err(|_| CryptoError::Rng)
}

pub fn random_bytes(n: usize) -> Result<Vec<u8>, CryptoError> {
    let mut v = vec![0u8; n];
    fill_random(&mut v)?;
    Ok(v)
}

pub fn random_key() -> Result<[u8; KEY_LEN], CryptoError> {
    let mut k = [0u8; KEY_LEN];
    fill_random(&mut k)?;
    Ok(k)
}

fn bound_aad(aad: &[u8]) -> Vec<u8> {
    let mut a = Vec::with_capacity(aad.len() + 1);
    a.push(AEAD_V2);
    a.extend_from_slice(aad);
    a
}

/// Seal with a fresh random 24-byte nonce. Returns an AEAD blob v2.
pub fn seal(key: &[u8; KEY_LEN], aad: &[u8], pt: &[u8]) -> Result<Vec<u8>, CryptoError> {
    seal_with_rng(fill_random, key, aad, pt)
}

/// `seal` with an injectable nonce source, so RNG failure propagation is testable.
pub fn seal_with_rng(
    rng: impl FnOnce(&mut [u8]) -> Result<(), CryptoError>,
    key: &[u8; KEY_LEN],
    aad: &[u8],
    pt: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let mut nonce = [0u8; NONCE_LEN];
    rng(&mut nonce)?;
    let ct = XChaCha20Poly1305::new(key.into())
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: pt,
                aad: &bound_aad(aad),
            },
        )
        .map_err(|_| CryptoError::Malformed)?;
    let mut out = Vec::with_capacity(AEAD_HEADER + ct.len());
    out.push(AEAD_V2);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Authenticate-then-decrypt; no plaintext is produced unless the tag verifies.
pub fn open(key: &[u8; KEY_LEN], aad: &[u8], blob: &[u8]) -> Result<Vec<u8>, CryptoError> {
    match blob.first() {
        None => return Err(CryptoError::Malformed),
        Some(&AEAD_V2) => {}
        Some(&v) => return Err(CryptoError::UnsupportedVersion(v)),
    }
    if blob.len() < AEAD_HEADER + TAG_LEN {
        return Err(CryptoError::Malformed);
    }
    let (nonce, ct) = blob[1..].split_at(NONCE_LEN);
    XChaCha20Poly1305::new(key.into())
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ct,
                aad: &bound_aad(aad),
            },
        )
        .map_err(|_| CryptoError::Auth)
}

pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC accepts any key length");
    m.update(data);
    m.finalize().into_bytes().into()
}

pub fn hkdf_sha256(
    salt: &[u8],
    ikm: &[u8],
    info: &[u8],
    len: usize,
) -> Result<Vec<u8>, CryptoError> {
    let mut out = vec![0u8; len];
    Hkdf::<Sha256>::new(Some(salt), ikm)
        .expand(info, &mut out)
        .map_err(|_| CryptoError::Malformed)?;
    Ok(out)
}

/// 32-byte subkey: HKDF-SHA256(salt="rei-os/v2", ikm, info).
pub fn derive_key(ikm: &[u8], info: &str) -> Zeroizing<[u8; KEY_LEN]> {
    let mut k = Zeroizing::new([0u8; KEY_LEN]);
    Hkdf::<Sha256>::new(Some(b"rei-os/v2"), ikm)
        .expand(info.as_bytes(), k.as_mut())
        .expect("32 bytes is a valid HKDF-SHA256 length");
    k
}

// ---------------------------------------------------------------- X25519

/// Fresh static X25519 device secret (clamped by x25519-dalek on use).
pub fn x25519_generate() -> Result<Zeroizing<[u8; 32]>, CryptoError> {
    let mut s = Zeroizing::new([0u8; 32]);
    fill_random(s.as_mut())?;
    Ok(s)
}

pub fn x25519_public(secret: &[u8; 32]) -> [u8; 32] {
    PublicKey::from(&StaticSecret::from(*secret)).to_bytes()
}

fn dh(secret: &StaticSecret, public: &[u8; 32]) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
    let shared = secret.diffie_hellman(&PublicKey::from(*public));
    if !shared.was_contributory() {
        return Err(CryptoError::WeakKey);
    }
    Ok(Zeroizing::new(shared.to_bytes()))
}

fn wrap_key(
    dh_e: &[u8; 32],
    dh_s: &[u8; 32],
    eph: &[u8; 32],
    sender: &[u8; 32],
    recipient: &[u8; 32],
) -> Zeroizing<[u8; KEY_LEN]> {
    let mut ikm = Zeroizing::new([0u8; 64]);
    ikm[..32].copy_from_slice(dh_e);
    ikm[32..].copy_from_slice(dh_s);
    let mut info = b"rei-os/wrap/v2".to_vec();
    info.extend_from_slice(eph);
    info.extend_from_slice(sender);
    info.extend_from_slice(recipient);
    let mut k = Zeroizing::new([0u8; KEY_LEN]);
    Hkdf::<Sha256>::new(None, ikm.as_ref())
        .expand(&info, k.as_mut())
        .expect("32 bytes is a valid HKDF-SHA256 length");
    k
}

/// Encrypt `pt` to `recipient_pub`, authenticated as the holder of `sender_secret`.
pub fn wrap_to(
    sender_secret: &[u8; 32],
    recipient_pub: &[u8; 32],
    aad: &[u8],
    pt: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let mut eph_bytes = x25519_generate()?;
    let eph = StaticSecret::from(*eph_bytes);
    eph_bytes.zeroize();
    let eph_pub = PublicKey::from(&eph).to_bytes();
    let sender = StaticSecret::from(*sender_secret);
    let sender_pub = PublicKey::from(&sender).to_bytes();
    let key = wrap_key(
        &*dh(&eph, recipient_pub)?,
        &*dh(&sender, recipient_pub)?,
        &eph_pub,
        &sender_pub,
        recipient_pub,
    );
    let mut out = Vec::with_capacity(WRAP_HEADER + AEAD_HEADER + pt.len() + TAG_LEN);
    out.push(WRAP_V2);
    out.extend_from_slice(&eph_pub);
    out.extend_from_slice(&sender_pub);
    out.extend_from_slice(&seal(&key, aad, pt)?);
    Ok(out)
}

/// Sender public key claimed by a wrap blob (checked against trusted peers before
/// `unwrap_from`; the claim is only proven once `unwrap_from` succeeds).
pub fn wrap_sender(blob: &[u8]) -> Result<[u8; 32], CryptoError> {
    match blob.first() {
        None => Err(CryptoError::Malformed),
        Some(&WRAP_V2) if blob.len() >= WRAP_HEADER => {
            Ok(blob[33..65].try_into().expect("32-byte slice"))
        }
        Some(&WRAP_V2) => Err(CryptoError::Malformed),
        Some(&v) => Err(CryptoError::UnsupportedVersion(v)),
    }
}

/// Decrypt a wrap addressed to `recipient_secret`, requiring it to come from
/// `expected_sender_pub`.
pub fn unwrap_from(
    recipient_secret: &[u8; 32],
    expected_sender_pub: &[u8; 32],
    aad: &[u8],
    blob: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    if &wrap_sender(blob)? != expected_sender_pub {
        return Err(CryptoError::Auth);
    }
    let eph_pub: [u8; 32] = blob[1..33].try_into().expect("32-byte slice");
    let me = StaticSecret::from(*recipient_secret);
    let my_pub = PublicKey::from(&me).to_bytes();
    let key = wrap_key(
        &*dh(&me, &eph_pub)?,
        &*dh(&me, expected_sender_pub)?,
        &eph_pub,
        expected_sender_pub,
        &my_pub,
    );
    open(&key, aad, &blob[WRAP_HEADER..])
}

// ---------------------------------------------------------------- encoding

pub fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn from_hex(s: &str) -> Result<Vec<u8>, CryptoError> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return Err(CryptoError::Malformed);
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| CryptoError::Malformed))
        .collect()
}

pub fn key_from_hex(s: &str) -> Result<Zeroizing<[u8; KEY_LEN]>, CryptoError> {
    let v = Zeroizing::new(from_hex(s)?);
    let arr: [u8; KEY_LEN] = v
        .as_slice()
        .try_into()
        .map_err(|_| CryptoError::Malformed)?;
    Ok(Zeroizing::new(arr))
}
