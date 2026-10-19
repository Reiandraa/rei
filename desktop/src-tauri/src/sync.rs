//! P11 encrypted outbox sync. Each outbox op is sealed (XChaCha20-Poly1305) with the
//! current epoch key; the relay stores only ciphertext plus routing metadata
//! (account, device, epoch, op_id). AAD binds every envelope to those fields, so a
//! relay cannot re-attribute, re-epoch or splice ops. Remote ops are applied in one
//! transaction with the idempotency ledger and cursor, so a crash at any boundary
//! either replays (deduped by op_id) or leaves no partial state.
//!
//! `Relay` is the actual local relay: a separate SQLite database whose every query
//! is scoped by account_id (the RLS model). The hosted relay is REQUIRES_USER_AUTH.
//! Revocation: a rotating device wraps the new epoch key to each trusted peer's
//! X25519 public key; peers whose public key it lacks are reported in
//! `RotateResult::unreachable` and must re-pair. A revoked device keeps every key
//! and plaintext it already had. Remaining gates: BUILD-BLOCKERS.md (sync).
use crate::crypto::{self, CryptoError, KEY_LEN};
use crate::db;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::Path;
use zeroize::{Zeroize, Zeroizing};

#[derive(Debug)]
pub enum SyncError {
    Db(rusqlite::Error),
    Crypto(CryptoError),
    /// Relay rejected the device (revoked/unknown) or the account scope.
    Forbidden,
    UnknownEpoch(u32),
    Invalid(String),
    Io(String),
}

impl From<rusqlite::Error> for SyncError {
    fn from(e: rusqlite::Error) -> Self {
        SyncError::Db(e)
    }
}
impl From<CryptoError> for SyncError {
    fn from(e: CryptoError) -> Self {
        SyncError::Crypto(e)
    }
}
impl From<db::DbError> for SyncError {
    fn from(e: db::DbError) -> Self {
        SyncError::Invalid(format!("{e:?}"))
    }
}

type Res<T> = Result<T, SyncError>;

/// Entity types allowed on the wire, mapped to their table. Anything else is
/// quarantined; table names never come from remote input.
const ENTITIES: &[(&str, &str)] = &[("task", "tasks"), ("event", "events")];

fn table_for(entity_type: &str) -> Option<&'static str> {
    ENTITIES
        .iter()
        .find(|(e, _)| *e == entity_type)
        .map(|(_, t)| *t)
}

/// SyncKeys serialization version. Older vault blobs (no/other `format`) are
/// rejected, never reinterpreted: they held v1 symmetric device keys.
pub const KEYS_FORMAT: u32 = 2;
/// Envelope/outbox `nonce` field value for v2: the 24-byte nonce lives inside the
/// versioned AEAD blob, so this field only names the format. Anything else (e.g. a
/// v1 hex nonce) is refused.
pub const ENVELOPE_FORMAT: &str = "xc20p1305-v2";

/// Device key material. Persist only through `vault::SecretStore` (SyncMasterKey).
/// Each device holds its own X25519 secret; peers are known only by public key, so
/// a revoked device cannot unwrap epoch keys addressed to anyone else.
#[derive(Clone, Serialize, Deserialize)]
pub struct SyncKeys {
    format: u32,
    pub account_id: String,
    pub device_id: String,
    device_secret: String,
    epochs: BTreeMap<u32, String>,
    pub current_epoch: u32,
    /// X25519 public keys of peers this device wraps rotated epochs for and
    /// accepts epoch wraps from.
    peers: BTreeMap<String, String>,
}

impl Drop for SyncKeys {
    fn drop(&mut self) {
        self.device_secret.zeroize();
        for v in self.epochs.values_mut() {
            v.zeroize();
        }
    }
}

impl SyncKeys {
    /// First device of an account: fresh epoch-1 key and device keypair.
    pub fn create(account_id: &str, device_id: &str) -> Res<Self> {
        let mut epochs = BTreeMap::new();
        epochs.insert(1, crypto::to_hex(&crypto::random_key()?));
        Ok(SyncKeys {
            format: KEYS_FORMAT,
            account_id: account_id.into(),
            device_id: device_id.into(),
            device_secret: crypto::to_hex(crypto::x25519_generate()?.as_ref()),
            epochs,
            current_epoch: 1,
            peers: BTreeMap::new(),
        })
    }
    fn epoch_key(&self, epoch: u32) -> Res<Zeroizing<[u8; KEY_LEN]>> {
        let hex = self
            .epochs
            .get(&epoch)
            .ok_or(SyncError::UnknownEpoch(epoch))?;
        Ok(crypto::key_from_hex(hex)?)
    }
    fn device_secret(&self) -> Res<Zeroizing<[u8; KEY_LEN]>> {
        Ok(crypto::key_from_hex(&self.device_secret)?)
    }
    pub fn public_key(&self) -> Res<String> {
        Ok(crypto::to_hex(&crypto::x25519_public(
            &*self.device_secret()?,
        )))
    }
    pub fn peer_ids(&self) -> Vec<String> {
        self.peers.keys().cloned().collect()
    }
    pub fn to_secret(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(serde_json::to_vec(self).expect("keys serialize"))
    }
    pub fn from_secret(b: &[u8]) -> Res<Self> {
        let v: Value =
            serde_json::from_slice(b).map_err(|_| SyncError::Invalid("sync keys".into()))?;
        match v.get("format").and_then(Value::as_u64) {
            Some(f) if f == KEYS_FORMAT as u64 => {}
            other => {
                return Err(SyncError::Invalid(format!(
                    "unsupported sync key format {other:?}; re-pair this device"
                )))
            }
        }
        serde_json::from_value(v).map_err(|_| SyncError::Invalid("sync keys".into()))
    }
}

fn aad(kind: &str, account: &str, device: &str, epoch: u32, id: &str) -> Vec<u8> {
    // Length-prefixed fields: no delimiter ambiguity between ids.
    let mut a = b"rei-sync/v2".to_vec();
    for f in [kind, account, device, &epoch.to_string(), id] {
        a.extend_from_slice(&(f.len() as u32).to_be_bytes());
        a.extend_from_slice(f.as_bytes());
    }
    a
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Envelope {
    pub op_id: String,
    pub account_id: String,
    pub device_id: String,
    pub epoch: u32,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Serialize, Deserialize)]
struct OpPayload {
    entity_type: String,
    entity_id: String,
    base_revision: i64,
    logical_seq: i64,
    row: Map<String, Value>,
}

// ---------------------------------------------------------------- local relay

pub struct Relay {
    conn: Connection,
    pub relay_id: String,
}

impl Relay {
    pub fn open(path: &Path) -> Res<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS relay_devices (account_id TEXT NOT NULL, device_id TEXT NOT NULL,
               revoked INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (account_id, device_id));
             CREATE TABLE IF NOT EXISTS relay_ops (seq INTEGER PRIMARY KEY AUTOINCREMENT,
               account_id TEXT NOT NULL, op_id TEXT NOT NULL, device_id TEXT NOT NULL,
               epoch INTEGER NOT NULL, nonce TEXT NOT NULL, ciphertext TEXT NOT NULL,
               UNIQUE (account_id, op_id));
             CREATE TABLE IF NOT EXISTS relay_wraps (account_id TEXT NOT NULL, epoch INTEGER NOT NULL,
               device_id TEXT NOT NULL, nonce TEXT NOT NULL, wrapped TEXT NOT NULL,
               PRIMARY KEY (account_id, epoch, device_id));",
        )?;
        Ok(Relay {
            conn,
            relay_id: path.display().to_string(),
        })
    }

    pub fn register_device(&self, account: &str, device: &str) -> Res<()> {
        let revoked: Option<i64> = self
            .conn
            .query_row(
                "SELECT revoked FROM relay_devices WHERE account_id=?1 AND device_id=?2",
                params![account, device],
                |r| r.get(0),
            )
            .optional()?;
        match revoked {
            Some(1) => Err(SyncError::Forbidden),
            Some(_) => Ok(()),
            None => {
                self.conn.execute(
                    "INSERT INTO relay_devices(account_id, device_id) VALUES (?1, ?2)",
                    params![account, device],
                )?;
                Ok(())
            }
        }
    }

    fn authorize(&self, account: &str, device: &str) -> Res<()> {
        let ok: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM relay_devices WHERE account_id=?1 AND device_id=?2 AND revoked=0",
                params![account, device],
                |r| r.get(0),
            )
            .optional()?;
        ok.map(|_| ()).ok_or(SyncError::Forbidden)
    }

    /// Idempotent: re-pushing an op_id returns the original sequence.
    pub fn push(&self, env: &Envelope) -> Res<i64> {
        self.authorize(&env.account_id, &env.device_id)?;
        self.conn.execute(
            "INSERT OR IGNORE INTO relay_ops(account_id, op_id, device_id, epoch, nonce, ciphertext)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![env.account_id, env.op_id, env.device_id, env.epoch, env.nonce, env.ciphertext],
        )?;
        Ok(self.conn.query_row(
            "SELECT seq FROM relay_ops WHERE account_id=?1 AND op_id=?2",
            params![env.account_id, env.op_id],
            |r| r.get(0),
        )?)
    }

    pub fn pull(&self, account: &str, device: &str, after: i64) -> Res<Vec<(i64, Envelope)>> {
        self.authorize(account, device)?;
        let mut st = self.conn.prepare(
            "SELECT seq, op_id, device_id, epoch, nonce, ciphertext FROM relay_ops
             WHERE account_id=?1 AND seq>?2 ORDER BY seq",
        )?;
        let rows = st.query_map(params![account, after], |r| {
            Ok((
                r.get(0)?,
                Envelope {
                    op_id: r.get(1)?,
                    account_id: account.to_string(),
                    device_id: r.get(2)?,
                    epoch: r.get(3)?,
                    nonce: r.get(4)?,
                    ciphertext: r.get(5)?,
                },
            ))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn revoke(&self, account: &str, by: &str, target: &str) -> Res<()> {
        self.authorize(account, by)?;
        self.conn.execute(
            "UPDATE relay_devices SET revoked=1 WHERE account_id=?1 AND device_id=?2",
            params![account, target],
        )?;
        Ok(())
    }

    fn post_wrap(
        &self,
        account: &str,
        by: &str,
        epoch: u32,
        to: &str,
        n: &str,
        w: &str,
    ) -> Res<()> {
        self.authorize(account, by)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO relay_wraps VALUES (?1, ?2, ?3, ?4, ?5)",
            params![account, epoch, to, n, w],
        )?;
        Ok(())
    }

    fn fetch_wrap(&self, account: &str, device: &str, epoch: u32) -> Res<(String, String)> {
        self.authorize(account, device)?;
        self.conn
            .query_row(
                "SELECT nonce, wrapped FROM relay_wraps WHERE account_id=?1 AND epoch=?2 AND device_id=?3",
                params![account, epoch, device],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or(SyncError::UnknownEpoch(epoch))
    }

    /// Test/inspection hook: raw stored ciphertext must never contain plaintext.
    pub fn raw_dump(&self) -> Res<String> {
        let mut st = self.conn.prepare("SELECT ciphertext FROM relay_ops")?;
        let v: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        Ok(v.join("\n"))
    }

    /// Test hook simulating a malicious relay rewriting a stored op.
    pub fn tamper(&self, op_id: &str, ciphertext: &str, epoch: Option<u32>) -> Res<()> {
        self.conn.execute(
            "UPDATE relay_ops SET ciphertext=?2, epoch=COALESCE(?3, epoch) WHERE op_id=?1",
            params![op_id, ciphertext, epoch],
        )?;
        Ok(())
    }
}

// ---------------------------------------------------------------- pairing

/// Pairing offers expire after this many seconds (INTERFACES §8: 10 min TTL).
pub const PAIRING_TTL_SECS: i64 = 600;

/// One-time pairing offer. `code` (128 random bits) is shown on the existing device
/// and must travel out of band (QR/typed), never via the relay; `blob` may travel
/// via the relay because it is sealed under a key derived from the code. The blob
/// carries epoch keys and peer PUBLIC keys only; the new device generates its own
/// X25519 secret and returns its public key in a code-authenticated confirmation.
/// INTERIM: this is not the X25519+SAS exchange specified in INTERFACES §8 (see
/// BUILD-BLOCKERS.md, sync); sync is not wired to any command or UI.
pub struct PairingOffer {
    pub code: String,
    pub new_device_id: String,
    pub expires_at: String,
    pub blob: Envelope,
}

fn pair_key(code: &str, purpose: &str) -> Zeroizing<[u8; KEY_LEN]> {
    crypto::derive_key(code.trim().as_bytes(), &format!("rei-os/pair/{purpose}/v2"))
}

fn ts(s: &str) -> Res<jiff::Timestamp> {
    s.parse()
        .map_err(|_| SyncError::Invalid(format!("timestamp {s}")))
}

pub fn pair_offer(
    keys: &SyncKeys,
    relay: &Relay,
    new_device_id: &str,
    now: &str,
) -> Res<PairingOffer> {
    if new_device_id == keys.device_id || keys.peers.contains_key(new_device_id) {
        return Err(SyncError::Invalid("device already paired".into()));
    }
    let code = crypto::to_hex(&crypto::random_bytes(16)?);
    let mut peers = keys.peers.clone();
    peers.insert(keys.device_id.clone(), keys.public_key()?);
    let theirs = SyncKeys {
        format: KEYS_FORMAT,
        account_id: keys.account_id.clone(),
        device_id: new_device_id.into(),
        device_secret: String::new(),
        epochs: keys.epochs.clone(),
        current_epoch: keys.current_epoch,
        peers,
    };
    let ad = aad("pair", &keys.account_id, new_device_id, 0, "offer");
    let ct = crypto::seal(&pair_key(&code, "offer"), &ad, &theirs.to_secret())?;
    let expires_at = ts(now)?
        .checked_add(jiff::SignedDuration::from_secs(PAIRING_TTL_SECS))
        .map_err(|_| SyncError::Invalid("expiry".into()))?
        .to_string();
    relay.register_device(&keys.account_id, new_device_id)?;
    Ok(PairingOffer {
        code,
        new_device_id: new_device_id.into(),
        expires_at,
        blob: Envelope {
            op_id: "pair-offer".into(),
            account_id: keys.account_id.clone(),
            device_id: new_device_id.into(),
            epoch: 0,
            nonce: ENVELOPE_FORMAT.into(),
            ciphertext: crypto::to_hex(&ct),
        },
    })
}

/// New device side: a wrong code fails authentication and yields no keys. Returns
/// the keys plus a confirmation carrying this device's fresh public key.
pub fn pair_accept(code: &str, blob: &Envelope) -> Res<(SyncKeys, Envelope)> {
    if blob.nonce != ENVELOPE_FORMAT {
        return Err(SyncError::Crypto(CryptoError::UnsupportedVersion(0)));
    }
    let ad = aad("pair", &blob.account_id, &blob.device_id, 0, "offer");
    let pt = Zeroizing::new(crypto::open(
        &pair_key(code, "offer"),
        &ad,
        &crypto::from_hex(&blob.ciphertext)?,
    )?);
    let mut keys = SyncKeys::from_secret(&pt)?;
    if keys.account_id != blob.account_id || keys.device_id != blob.device_id {
        return Err(SyncError::Invalid("pairing identity mismatch".into()));
    }
    keys.device_secret = crypto::to_hex(crypto::x25519_generate()?.as_ref());
    let cad = aad("pair", &keys.account_id, &keys.device_id, 0, "confirm");
    let confirm = crypto::seal(
        &pair_key(code, "confirm"),
        &cad,
        keys.public_key()?.as_bytes(),
    )?;
    let env = Envelope {
        op_id: "pair-confirm".into(),
        account_id: keys.account_id.clone(),
        device_id: keys.device_id.clone(),
        epoch: 0,
        nonce: ENVELOPE_FORMAT.into(),
        ciphertext: crypto::to_hex(&confirm),
    };
    Ok((keys, env))
}

/// Existing device side: verifies the code-authenticated confirmation within the
/// TTL and records the new device's public key. Consumes the offer (single use).
pub fn pair_complete(
    keys: &mut SyncKeys,
    local: &Connection,
    offer: PairingOffer,
    confirm: &Envelope,
    name: &str,
    now: &str,
) -> Res<()> {
    if ts(now)? > ts(&offer.expires_at)? {
        return Err(SyncError::Invalid("pairing offer expired".into()));
    }
    if confirm.nonce != ENVELOPE_FORMAT
        || confirm.account_id != keys.account_id
        || confirm.device_id != offer.new_device_id
        || keys.peers.contains_key(&offer.new_device_id)
    {
        return Err(SyncError::Invalid("pairing confirmation mismatch".into()));
    }
    let cad = aad("pair", &keys.account_id, &offer.new_device_id, 0, "confirm");
    let pk = crypto::open(
        &pair_key(&offer.code, "confirm"),
        &cad,
        &crypto::from_hex(&confirm.ciphertext)?,
    )?;
    let pk = String::from_utf8(pk).map_err(|_| SyncError::Invalid("public key".into()))?;
    if crypto::from_hex(&pk)?.len() != 32 {
        return Err(SyncError::Invalid("public key".into()));
    }
    keys.peers.insert(offer.new_device_id.clone(), pk);
    local.execute(
        "INSERT OR REPLACE INTO sync_devices(device_id, name, paired_at) VALUES (?1, ?2, ?3)",
        params![offer.new_device_id, name, now],
    )?;
    Ok(())
}

// ---------------------------------------------------------------- revocation

pub struct RotateResult {
    pub new_epoch: u32,
    pub wrapped_for: Vec<String>,
    pub unreachable: Vec<String>,
}

/// Wrap payload: the new epoch key plus the devices revoked so far, so recipients
/// stop trusting those devices' public keys for future wraps.
#[derive(Serialize, Deserialize)]
struct WrapPayload {
    key: String,
    revoked: Vec<String>,
}

pub fn revoke_device(
    keys: &mut SyncKeys,
    relay: &Relay,
    local: &Connection,
    target: &str,
    now: &str,
) -> Res<RotateResult> {
    if target == keys.device_id {
        return Err(SyncError::Invalid("cannot revoke self".into()));
    }
    // Generate before any state change so an RNG failure leaves nothing half-done.
    let new_key = Zeroizing::new(crypto::random_key()?);
    relay.revoke(&keys.account_id, &keys.device_id, target)?;
    local.execute(
        "UPDATE sync_devices SET revoked_at=?2 WHERE device_id=?1",
        params![target, now],
    )?;
    keys.peers.remove(target);
    let mut revoked: Vec<String> = local
        .prepare("SELECT device_id FROM sync_devices WHERE revoked_at IS NOT NULL")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    if !revoked.iter().any(|d| d == target) {
        revoked.push(target.into());
    }
    let new_epoch = keys.current_epoch + 1;
    let payload = Zeroizing::new(
        serde_json::to_vec(&WrapPayload {
            key: crypto::to_hex(new_key.as_ref()),
            revoked: revoked.clone(),
        })
        .map_err(|e| SyncError::Invalid(e.to_string()))?,
    );
    let me = keys.device_secret()?;
    let mut wrapped_for = Vec::new();
    for (peer, pk) in &keys.peers {
        if revoked.contains(peer) {
            continue;
        }
        let pk: [u8; 32] = crypto::from_hex(pk)?
            .try_into()
            .map_err(|_| SyncError::Crypto(CryptoError::Malformed))?;
        let ad = aad("wrap", &keys.account_id, peer, new_epoch, "epoch");
        let w = crypto::wrap_to(&me, &pk, &ad, &payload)?;
        relay.post_wrap(
            &keys.account_id,
            &keys.device_id,
            new_epoch,
            peer,
            ENVELOPE_FORMAT,
            &crypto::to_hex(&w),
        )?;
        wrapped_for.push(peer.clone());
    }
    let unreachable: Vec<String> = local
        .prepare("SELECT device_id FROM sync_devices WHERE revoked_at IS NULL")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|d| d != &keys.device_id && !wrapped_for.contains(d))
        .collect();
    keys.epochs
        .insert(new_epoch, crypto::to_hex(new_key.as_ref()));
    keys.current_epoch = new_epoch;
    audit(local, now, "sync.revoke", "device", target, "ok")?;
    Ok(RotateResult {
        new_epoch,
        wrapped_for,
        unreachable,
    })
}

/// Learn an epoch key from a wrap addressed to this device. Only wraps that
/// authenticate as a currently trusted peer are accepted.
fn learn_epoch(keys: &mut SyncKeys, relay: &Relay, epoch: u32) -> Res<()> {
    let (fmt, w) = relay.fetch_wrap(&keys.account_id, &keys.device_id, epoch)?;
    if fmt != ENVELOPE_FORMAT {
        return Err(SyncError::Crypto(CryptoError::UnsupportedVersion(0)));
    }
    let w = crypto::from_hex(&w)?;
    let sender = crypto::wrap_sender(&w)?;
    if !keys.peers.values().any(|pk| pk == &crypto::to_hex(&sender)) {
        return Err(SyncError::Crypto(CryptoError::Auth));
    }
    let ad = aad("wrap", &keys.account_id, &keys.device_id, epoch, "epoch");
    let pt = Zeroizing::new(crypto::unwrap_from(
        &*keys.device_secret()?,
        &sender,
        &ad,
        &w,
    )?);
    let p: WrapPayload =
        serde_json::from_slice(&pt).map_err(|_| SyncError::Invalid("wrap".into()))?;
    let k = crypto::key_from_hex(&p.key)?;
    for d in &p.revoked {
        keys.peers.remove(d);
    }
    keys.epochs.insert(epoch, crypto::to_hex(k.as_ref()));
    keys.current_epoch = keys.current_epoch.max(epoch);
    Ok(())
}

fn audit(c: &Connection, now: &str, action: &str, tt: &str, tid: &str, outcome: &str) -> Res<()> {
    c.execute(
        "INSERT INTO audit_events(id, at, actor, action, target_type, target_id, outcome)
         VALUES (?1, ?2, 'Sync', ?3, ?4, ?5, ?6)",
        params![
            uuid::Uuid::new_v4().to_string(),
            now,
            action,
            tt,
            tid,
            outcome
        ],
    )?;
    Ok(())
}

fn row_json(c: &Connection, table: &str, id: &str) -> Res<Option<Map<String, Value>>> {
    let mut st = c.prepare(&format!("SELECT * FROM {table} WHERE id=?1"))?;
    let names: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
    let row = st
        .query_row([id], |r| {
            let mut m = Map::new();
            for (i, n) in names.iter().enumerate() {
                use rusqlite::types::ValueRef as V;
                let v = match r.get_ref(i)? {
                    V::Null => Value::Null,
                    V::Integer(x) => Value::from(x),
                    V::Real(x) => Value::from(x),
                    V::Text(t) => Value::from(String::from_utf8_lossy(t).to_string()),
                    V::Blob(_) => Value::Null,
                };
                m.insert(n.clone(), v);
            }
            Ok(m)
        })
        .optional()?;
    Ok(row)
}

/// Seal pending ops (snapshotting the current row) under the current epoch.
pub fn seal_pending(c: &mut Connection, keys: &SyncKeys) -> Res<usize> {
    let key = keys.epoch_key(keys.current_epoch)?;
    let tx = c.transaction()?;
    let ops: Vec<(String, String, String, i64, i64)> = tx
        .prepare(
            "SELECT op_id, entity_type, entity_id, base_revision, logical_seq FROM outbox
             WHERE status='Pending' AND ciphertext IS NULL ORDER BY logical_seq",
        )?
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<Result<_, _>>()?;
    for (op_id, et, eid, base, seq) in &ops {
        let Some(table) = table_for(et) else {
            tx.execute(
                "UPDATE outbox SET status='Quarantined' WHERE op_id=?1",
                [op_id],
            )?;
            continue;
        };
        let row = row_json(&tx, table, eid)?.unwrap_or_default();
        let pt = serde_json::to_vec(&OpPayload {
            entity_type: et.clone(),
            entity_id: eid.clone(),
            base_revision: *base,
            logical_seq: *seq,
            row,
        })
        .map_err(|e| SyncError::Invalid(e.to_string()))?;
        let ad = aad(
            "op",
            &keys.account_id,
            &keys.device_id,
            keys.current_epoch,
            op_id,
        );
        tx.execute(
            "UPDATE outbox SET ciphertext=?2, nonce=?3, epoch=?4, origin_device_id=?5 WHERE op_id=?1",
            params![op_id, crypto::seal(&key, &ad, &pt)?, ENVELOPE_FORMAT.as_bytes(), keys.current_epoch, keys.device_id],
        )?;
    }
    tx.commit()?;
    Ok(ops.len())
}

/// Push sealed ops. Status moves Pending -> Acked only after the relay returns a
/// sequence; a crash in between re-pushes the same op_id, which the relay dedupes.
pub fn push(c: &Connection, keys: &SyncKeys, relay: &Relay) -> Res<usize> {
    let ops: Vec<(String, Vec<u8>, Vec<u8>, u32)> = c
        .prepare(
            "SELECT op_id, ciphertext, nonce, epoch FROM outbox
             WHERE status IN ('Pending','Sent') AND ciphertext IS NOT NULL ORDER BY logical_seq",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<_, _>>()?;
    for (op_id, ct, nonce, epoch) in &ops {
        if nonce.as_slice() != ENVELOPE_FORMAT.as_bytes() {
            // Legacy v1 ciphertext: never pushed, never deleted; the source row
            // stays in place and the op is visibly quarantined.
            c.execute(
                "UPDATE outbox SET status='Quarantined' WHERE op_id=?1",
                [op_id],
            )?;
            continue;
        }
        c.execute("UPDATE outbox SET status='Sent' WHERE op_id=?1", [op_id])?;
        relay.push(&Envelope {
            op_id: op_id.clone(),
            account_id: keys.account_id.clone(),
            device_id: keys.device_id.clone(),
            epoch: *epoch,
            nonce: ENVELOPE_FORMAT.into(),
            ciphertext: crypto::to_hex(ct),
        })?;
        c.execute("UPDATE outbox SET status='Acked' WHERE op_id=?1", [op_id])?;
    }
    Ok(ops.len())
}

#[derive(Debug, Default, PartialEq)]
pub struct PullReport {
    pub applied: usize,
    pub duplicates: usize,
    pub stale: usize,
    pub conflicts: usize,
    pub quarantined: usize,
}

fn open_env(keys: &mut SyncKeys, relay: &Relay, env: &Envelope) -> Res<OpPayload> {
    if !keys.epochs.contains_key(&env.epoch) {
        learn_epoch(keys, relay, env.epoch)?;
    }
    if env.nonce != ENVELOPE_FORMAT {
        return Err(SyncError::Crypto(CryptoError::UnsupportedVersion(0)));
    }
    let ad = aad("op", &env.account_id, &env.device_id, env.epoch, &env.op_id);
    let pt = crypto::open(
        &*keys.epoch_key(env.epoch)?,
        &ad,
        &crypto::from_hex(&env.ciphertext)?,
    )?;
    let p: OpPayload =
        serde_json::from_slice(&pt).map_err(|_| SyncError::Invalid("payload".into()))?;
    if table_for(&p.entity_type).is_none()
        || p.row.get("id") != Some(&Value::from(p.entity_id.clone()))
    {
        return Err(SyncError::Invalid("entity".into()));
    }
    Ok(p)
}

fn rev(m: &Map<String, Value>) -> i64 {
    m.get("revision").and_then(Value::as_i64).unwrap_or(0)
}

/// Deterministic, symmetric tie-break so both devices converge: a live row beats a
/// tombstone (delete-vs-edit keeps the user's edit), then later updated_at, then
/// origin_device_id.
fn remote_wins_tie(local: &Map<String, Value>, remote: &Map<String, Value>) -> bool {
    let key = |m: &Map<String, Value>| {
        (
            m.get("deleted_at").is_none_or(Value::is_null),
            m.get("updated_at")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            m.get("origin_device_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        )
    };
    key(remote) > key(local)
}

fn upsert(tx: &Connection, table: &str, row: &Map<String, Value>) -> Res<()> {
    let cols: Vec<String> = tx
        .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    if row.keys().any(|k| !cols.contains(k)) {
        return Err(SyncError::Invalid("unknown column".into()));
    }
    let keys: Vec<&String> = row.keys().collect();
    let names = keys
        .iter()
        .map(|k| k.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let marks = (1..=keys.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let sets = keys
        .iter()
        .filter(|k| k.as_str() != "id")
        .map(|k| format!("{k}=excluded.{k}"))
        .collect::<Vec<_>>()
        .join(",");
    let vals: Vec<rusqlite::types::Value> = keys
        .iter()
        .map(|k| match &row[k.as_str()] {
            Value::Null => rusqlite::types::Value::Null,
            Value::Number(n) if n.is_i64() => {
                rusqlite::types::Value::Integer(n.as_i64().unwrap_or(0))
            }
            Value::Number(n) => rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0)),
            Value::String(s) => rusqlite::types::Value::Text(s.clone()),
            other => rusqlite::types::Value::Text(other.to_string()),
        })
        .collect();
    tx.execute(
        &format!(
            "INSERT INTO {table}({names}) VALUES ({marks}) ON CONFLICT(id) DO UPDATE SET {sets}"
        ),
        rusqlite::params_from_iter(vals),
    )?;
    Ok(())
}

fn ledger(tx: &Connection, env: &Envelope, seq: i64, now: &str, outcome: &str) -> Res<()> {
    tx.execute(
        "INSERT INTO sync_applied(op_id, origin_device_id, relay_seq, applied_at, outcome) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![env.op_id, env.device_id, seq, now, outcome],
    )?;
    Ok(())
}

/// Pull and apply remote ops. Each op commits atomically with ledger + cursor.
/// Authentication/shape failures are quarantined (retry cannot fix them); a
/// constraint failure (e.g. FK not yet satisfied) stops the pull without advancing.
pub fn pull(c: &mut Connection, keys: &mut SyncKeys, relay: &Relay, now: &str) -> Res<PullReport> {
    let mut rep = PullReport::default();
    let after: i64 = c
        .query_row(
            "SELECT last_seq FROM sync_cursor WHERE relay_id=?1",
            [&relay.relay_id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0);
    for (seq, env) in relay.pull(&keys.account_id, &keys.device_id, after)? {
        let tx = c.transaction()?;
        let seen: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM sync_applied WHERE op_id=?1",
                [&env.op_id],
                |r| r.get(0),
            )
            .optional()?;
        if env.device_id == keys.device_id || seen.is_some() {
            if seen.is_some() {
                rep.duplicates += 1;
            }
        } else {
            match open_env(keys, relay, &env) {
                Err(SyncError::Crypto(_))
                | Err(SyncError::Invalid(_))
                | Err(SyncError::UnknownEpoch(_)) => {
                    ledger(&tx, &env, seq, now, "quarantined")?;
                    audit(&tx, now, "sync.apply", "op", &env.op_id, "quarantined")?;
                    rep.quarantined += 1;
                }
                Err(e) => return Err(e),
                Ok(p) => {
                    let outcome = apply(&tx, &p, now)?;
                    match outcome {
                        "applied" => rep.applied += 1,
                        "stale" => rep.stale += 1,
                        _ => rep.conflicts += 1,
                    }
                    ledger(&tx, &env, seq, now, outcome)?;
                }
            }
        }
        tx.execute(
            "INSERT INTO sync_cursor(relay_id, last_seq) VALUES (?1, ?2)
             ON CONFLICT(relay_id) DO UPDATE SET last_seq=excluded.last_seq",
            params![relay.relay_id, seq],
        )?;
        tx.commit()?;
    }
    Ok(rep)
}

fn apply(tx: &Connection, p: &OpPayload, now: &str) -> Res<&'static str> {
    let table = table_for(&p.entity_type).ok_or_else(|| SyncError::Invalid("entity".into()))?;
    let remote = &p.row;
    let Some(local) = row_json(tx, table, &p.entity_id)? else {
        upsert(tx, table, remote)?;
        return Ok("applied");
    };
    if &local == remote {
        return Ok("stale");
    }
    let pending: Option<i64> = tx
        .query_row(
            "SELECT 1 FROM outbox WHERE entity_type=?1 AND entity_id=?2 AND status IN ('Pending','Sent') LIMIT 1",
            params![p.entity_type, p.entity_id],
            |r| r.get(0),
        )
        .optional()?;
    let (lr, rr) = (rev(&local), rev(remote));
    let remote_wins = rr > lr || (rr == lr && remote_wins_tie(&local, remote));
    let concurrent = rr == lr || pending.is_some();
    if remote_wins {
        upsert(tx, table, remote)?;
    }
    if !concurrent {
        return Ok(if remote_wins { "applied" } else { "stale" });
    }
    tx.execute(
        "INSERT INTO sync_conflicts(id, entity_type, entity_id, local_json, remote_json, winner, reason, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            uuid::Uuid::new_v4().to_string(),
            p.entity_type,
            p.entity_id,
            Value::Object(local).to_string(),
            Value::Object(remote.clone()).to_string(),
            if remote_wins { "remote" } else { "local" },
            if rr == lr { "concurrent_same_revision" } else { "concurrent_pending_local" },
            now
        ],
    )?;
    audit(
        tx,
        now,
        "sync.conflict",
        &p.entity_type,
        &p.entity_id,
        "recorded",
    )?;
    Ok(if remote_wins {
        "conflict_remote"
    } else {
        "conflict_local"
    })
}

// ---------------------------------------------------------------- backup/restore

const MAGIC: &[u8; 8] = b"REIBK02\0";

/// High-entropy recovery secret (160 bits); shown once, never stored by the app.
pub fn new_recovery_code() -> Res<String> {
    Ok(crypto::to_hex(&crypto::random_bytes(20)?))
}

/// Consistent snapshot (`VACUUM INTO`, safe during concurrent WAL writes) plus the
/// sync keys, sealed under a key derived from the recovery code.
pub fn backup(c: &Connection, keys: Option<&SyncKeys>, recovery: &str, out: &Path) -> Res<()> {
    let snap = out.with_extension("snapshot.tmp");
    let _ = std::fs::remove_file(&snap);
    c.execute("VACUUM INTO ?1", [snap.to_string_lossy()])?;
    let db_bytes = std::fs::read(&snap).map_err(|e| SyncError::Io(e.to_string()))?;
    std::fs::remove_file(&snap).map_err(|e| SyncError::Io(e.to_string()))?;
    let keys_json = keys.map(|k| k.to_secret()).unwrap_or_default();
    let mut pt = (keys_json.len() as u32).to_le_bytes().to_vec();
    pt.extend_from_slice(&keys_json);
    pt.extend_from_slice(&db_bytes);
    let version = db::schema_version(c)?;
    let ad = aad("backup", "", "", version, "db");
    let mut file = MAGIC.to_vec();
    file.extend_from_slice(&version.to_le_bytes());
    file.extend(crypto::seal(
        &crypto::derive_key(recovery.trim().as_bytes(), "backup"),
        &ad,
        &pt,
    )?);
    std::fs::write(out, file).map_err(|e| SyncError::Io(e.to_string()))
}

/// Restore into `dest` (which must not be open). Verifies the tag before writing
/// anything; migrates older schemas; refuses newer ones. A wrong recovery code or a
/// tampered file leaves `dest` untouched.
pub fn restore(input: &Path, recovery: &str, dest: &Path) -> Res<Option<SyncKeys>> {
    let file = std::fs::read(input).map_err(|e| SyncError::Io(e.to_string()))?;
    if file.len() < 12 || &file[..8] != MAGIC {
        return Err(SyncError::Invalid("not a REI backup".into()));
    }
    let version = u32::from_le_bytes([file[8], file[9], file[10], file[11]]);
    if version > db::supported_version() {
        return Err(SyncError::Invalid("backup from newer app version".into()));
    }
    let ad = aad("backup", "", "", version, "db");
    let pt = Zeroizing::new(crypto::open(
        &crypto::derive_key(recovery.trim().as_bytes(), "backup"),
        &ad,
        &file[12..],
    )?);
    if pt.len() < 4 {
        return Err(SyncError::Invalid("backup payload".into()));
    }
    let klen = u32::from_le_bytes([pt[0], pt[1], pt[2], pt[3]]) as usize;
    if pt.len() - 4 < klen {
        return Err(SyncError::Invalid("backup payload".into()));
    }
    let keys = if klen == 0 {
        None
    } else {
        Some(SyncKeys::from_secret(&pt[4..4 + klen])?)
    };
    let tmp = dest.with_extension("restore.tmp");
    std::fs::write(&tmp, &pt[4 + klen..]).map_err(|e| SyncError::Io(e.to_string()))?;
    {
        let mut conn = db::open(&tmp)?;
        db::migrate(&mut conn)?;
        let ok: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if ok != "ok" {
            return Err(SyncError::Invalid("integrity".into()));
        }
    }
    for ext in ["sqlite-wal", "sqlite-shm"] {
        let _ = std::fs::remove_file(dest.with_extension(ext));
    }
    std::fs::rename(&tmp, dest).map_err(|e| SyncError::Io(e.to_string()))?;
    Ok(keys)
}
