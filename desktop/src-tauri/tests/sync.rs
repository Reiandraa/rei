//! sync phase tests (P11): KDF/MAC/X25519 vectors, XChaCha20-Poly1305 v2 envelope
//! tamper/truncation/version/RNG-failure checks, two independent SQLite clients over an
//! actual local SQLite relay, conflicts, tamper/AAD failures, revocation with epoch
//! rotation, pairing, crash/replay, backup during WAL writes and restore. All data is
//! SYNTHETIC; no hosted relay, account or real device pair is exercised here.
use rei_os_lib::crypto as k;
use rei_os_lib::db;
use rei_os_lib::planner::{self, Ctx};
use rei_os_lib::sync::{self as s, PullReport, Relay, SyncError, SyncKeys};
use rusqlite::Connection;
use serde_json::json;
use std::path::Path;

fn hex(s: &str) -> Vec<u8> {
    k::from_hex(&s.replace([' ', ':'], "")).unwrap()
}

#[test]
fn kdf_mac_and_x25519_vectors() {
    // RFC 4231 test case 2, RFC 5869 test case 1, RFC 7748 section 6.1 (Alice).
    assert_eq!(
        k::hmac_sha256(b"Jefe", b"what do ya want for nothing?").to_vec(),
        hex("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")
    );
    let okm = k::hkdf_sha256(
        &hex("000102030405060708090a0b0c"),
        &[0x0b; 22],
        &hex("f0f1f2f3f4f5f6f7f8f9"),
        42,
    )
    .unwrap();
    assert_eq!(
        okm,
        hex("3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865")
    );
    let alice: [u8; 32] = hex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a")
        .try_into()
        .unwrap();
    assert_eq!(
        k::x25519_public(&alice).to_vec(),
        hex("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a")
    );
}

#[test]
fn aead_v2_rejects_tamper_truncation_wrong_key_aad_and_version() {
    let key = k::random_key().unwrap();
    let blob = k::seal(&key, b"aad", b"SYNTHETIC hello").unwrap();
    assert_eq!(blob[0], k::AEAD_V2);
    assert_eq!(blob.len(), 1 + k::NONCE_LEN + 15 + k::TAG_LEN);
    assert_eq!(k::open(&key, b"aad", &blob).unwrap(), b"SYNTHETIC hello");
    assert_eq!(k::open(&key, b"other", &blob), Err(k::CryptoError::Auth));
    let other = k::random_key().unwrap();
    assert_eq!(k::open(&other, b"aad", &blob), Err(k::CryptoError::Auth));
    // Every byte after the version (nonce, ciphertext, tag) is authenticated.
    for i in 1..blob.len() {
        let mut t = blob.clone();
        t[i] ^= 0x01;
        assert_eq!(
            k::open(&key, b"aad", &t),
            Err(k::CryptoError::Auth),
            "byte {i}"
        );
    }
    let mut v = blob.clone();
    v[0] = 0x01;
    assert_eq!(
        k::open(&key, b"aad", &v),
        Err(k::CryptoError::UnsupportedVersion(1))
    );
    // Truncation at every length fails closed, never panics or yields plaintext.
    for n in 0..blob.len() {
        assert!(k::open(&key, b"aad", &blob[..n]).is_err(), "len {n}");
    }
    assert_eq!(k::open(&key, b"aad", &[]), Err(k::CryptoError::Malformed));
    let mut ext = blob.clone();
    ext.push(0);
    assert_eq!(k::open(&key, b"aad", &ext), Err(k::CryptoError::Auth));
    // Repeated sealing of identical input never reuses a nonce.
    let blobs: std::collections::HashSet<_> = (0..5000)
        .map(|_| k::seal(&key, b"aad", b"same").unwrap()[1..1 + k::NONCE_LEN].to_vec())
        .collect();
    assert_eq!(blobs.len(), 5000, "random nonces must not repeat");
}

#[test]
fn rng_failure_propagates_and_produces_no_ciphertext() {
    let key = [7u8; 32];
    assert_eq!(
        k::seal_with_rng(|_| Err(k::CryptoError::Rng), &key, b"a", b"p"),
        Err(k::CryptoError::Rng)
    );
    // An injected fixed nonce is used verbatim (shows the nonce comes only from rng).
    let b = k::seal_with_rng(
        |n| {
            n.fill(9);
            Ok(())
        },
        &key,
        b"a",
        b"p",
    )
    .unwrap();
    assert_eq!(&b[1..25], &[9u8; 24]);
    assert_eq!(k::open(&key, b"a", &b).unwrap(), b"p");
}

#[test]
fn x25519_wrap_authenticates_sender_and_recipient() {
    let (s, r, x) = (
        k::x25519_generate().unwrap(),
        k::x25519_generate().unwrap(),
        k::x25519_generate().unwrap(),
    );
    let (sp, rp, xp) = (
        k::x25519_public(&s),
        k::x25519_public(&r),
        k::x25519_public(&x),
    );
    let w = k::wrap_to(&s, &rp, b"ad", b"SYNTHETIC epoch key").unwrap();
    assert_eq!(k::wrap_sender(&w).unwrap(), sp);
    assert_eq!(
        k::unwrap_from(&r, &sp, b"ad", &w).unwrap(),
        b"SYNTHETIC epoch key"
    );
    // Third party (e.g. a revoked device) cannot unwrap a wrap addressed to r.
    assert_eq!(
        k::unwrap_from(&x, &sp, b"ad", &w),
        Err(k::CryptoError::Auth)
    );
    // A wrap forged by x but labelled with s's public key fails.
    let mut forged = k::wrap_to(&x, &rp, b"ad", b"evil").unwrap();
    forged[33..65].copy_from_slice(&sp);
    assert_eq!(
        k::unwrap_from(&r, &sp, b"ad", &forged),
        Err(k::CryptoError::Auth)
    );
    assert_eq!(
        k::unwrap_from(&r, &xp, b"ad", &w),
        Err(k::CryptoError::Auth)
    );
    assert_eq!(
        k::unwrap_from(&r, &sp, b"other", &w),
        Err(k::CryptoError::Auth)
    );
    for i in 1..w.len() {
        let mut t = w.clone();
        t[i] ^= 0x01;
        assert!(k::unwrap_from(&r, &sp, b"ad", &t).is_err(), "byte {i}");
    }
    let mut v = w.clone();
    v[0] = 0x11;
    assert_eq!(
        k::unwrap_from(&r, &sp, b"ad", &v),
        Err(k::CryptoError::UnsupportedVersion(0x11))
    );
    assert!(k::unwrap_from(&r, &sp, b"ad", &w[..60]).is_err());
    assert_eq!(
        k::wrap_to(&s, &[0u8; 32], b"ad", b"k"),
        Err(k::CryptoError::WeakKey)
    );
}

#[test]
fn legacy_v1_key_blob_is_rejected_not_reinterpreted() {
    let v1 = json!({"account_id":"acct-1","device_id":"dev-a","device_key":"00","epochs":{},"current_epoch":1,"peers":{}});
    assert!(matches!(
        SyncKeys::from_secret(&serde_json::to_vec(&v1).unwrap()),
        Err(SyncError::Invalid(_))
    ));
    let keys = SyncKeys::create("acct-1", "dev-a").unwrap();
    assert!(SyncKeys::from_secret(&keys.to_secret()).is_ok());
}

struct Client {
    conn: Connection,
    keys: SyncKeys,
}

impl Client {
    fn ctx(&self) -> (String, String) {
        (self.keys.device_id.clone(), "2026-10-19T08:00:00Z".into())
    }
    fn create(&mut self, title: &str) -> String {
        let (d, n) = self.ctx();
        let t = planner::task_create(
            &mut self.conn,
            &Ctx {
                device_id: &d,
                now: &n,
            },
            serde_json::from_value(json!({"title": title})).unwrap(),
        )
        .unwrap();
        t.id
    }
    fn rename(&mut self, id: &str, title: &str, now: &str) {
        let d = self.keys.device_id.clone();
        let r = rev(&self.conn, id);
        planner::task_update(
            &mut self.conn,
            &Ctx { device_id: &d, now },
            serde_json::from_value(json!({"id": id, "expected_revision": r, "title": title}))
                .unwrap(),
        )
        .unwrap();
    }
    fn delete(&mut self, id: &str, now: &str) {
        let d = self.keys.device_id.clone();
        let r = rev(&self.conn, id);
        planner::task_delete(
            &mut self.conn,
            &Ctx { device_id: &d, now },
            serde_json::from_value(json!({"id": id, "expected_revision": r, "confirm": true}))
                .unwrap(),
        )
        .unwrap();
    }
    fn send(&mut self, relay: &Relay) {
        s::seal_pending(&mut self.conn, &self.keys).unwrap();
        s::push(&self.conn, &self.keys, relay).unwrap();
    }
    fn recv(&mut self, relay: &Relay) -> PullReport {
        s::pull(
            &mut self.conn,
            &mut self.keys,
            relay,
            "2026-10-19T09:00:00Z",
        )
        .unwrap()
    }
    fn title(&self, id: &str) -> String {
        self.conn
            .query_row("SELECT title FROM tasks WHERE id=?1", [id], |r| r.get(0))
            .unwrap()
    }
}

fn rev(c: &Connection, id: &str) -> u64 {
    c.query_row("SELECT revision FROM tasks WHERE id=?1", [id], |r| {
        r.get::<_, i64>(0)
    })
    .unwrap() as u64
}

fn open_db(p: &Path) -> Connection {
    let mut c = db::open(p).unwrap();
    db::migrate(&mut c).unwrap();
    c
}

/// Two desktop clients with separate SQLite files, paired through the relay.
fn setup(dir: &Path) -> (Relay, Client, Client) {
    let relay = Relay::open(&dir.join("relay.sqlite")).unwrap();
    let a_conn = open_db(&dir.join("a.sqlite"));
    let mut a_keys = SyncKeys::create("acct-1", "dev-a").unwrap();
    relay.register_device("acct-1", "dev-a").unwrap();
    let offer = s::pair_offer(&a_keys, &relay, "dev-b", "2026-10-19T08:00:00Z").unwrap();
    assert!(matches!(
        s::pair_accept("00000000000000000000000000000000", &offer.blob),
        Err(SyncError::Crypto(_))
    ));
    let (b_keys, confirm) = s::pair_accept(&offer.code, &offer.blob).unwrap();
    // A confirmation not produced with the code is rejected.
    let mut forged = confirm.clone();
    forged.ciphertext = k::to_hex(&k::seal(&[1u8; 32], b"x", b"00").unwrap());
    let forged_offer = s::PairingOffer {
        code: offer.code.clone(),
        new_device_id: offer.new_device_id.clone(),
        expires_at: offer.expires_at.clone(),
        blob: offer.blob.clone(),
    };
    assert!(s::pair_complete(
        &mut a_keys,
        &a_conn,
        forged_offer,
        &forged,
        "Mac",
        "2026-10-19T08:01:00Z"
    )
    .is_err());
    s::pair_complete(
        &mut a_keys,
        &a_conn,
        offer,
        &confirm,
        "Mac",
        "2026-10-19T08:01:00Z",
    )
    .unwrap();
    assert_eq!(a_keys.peer_ids(), vec!["dev-b".to_string()]);
    assert_eq!(b_keys.peer_ids(), vec!["dev-a".to_string()]);
    // Re-offering an already paired device is refused.
    assert!(s::pair_offer(&a_keys, &relay, "dev-b", "2026-10-19T08:02:00Z").is_err());
    let b_conn = open_db(&dir.join("b.sqlite"));
    (
        relay,
        Client {
            conn: a_conn,
            keys: a_keys,
        },
        Client {
            conn: b_conn,
            keys: b_keys,
        },
    )
}

#[test]
fn two_clients_offline_edits_converge_and_relay_sees_only_ciphertext() {
    let dir = tempfile::tempdir().unwrap();
    let (relay, mut a, mut b) = setup(dir.path());
    let t1 = a.create("SYNTHETIC secret dentist appointment");
    let t2 = b.create("SYNTHETIC buy chalk");
    a.send(&relay);
    b.send(&relay);
    assert!(!relay.raw_dump().unwrap().contains("dentist"));
    let raw = relay.raw_dump().unwrap();
    assert!(
        !String::from_utf8_lossy(&k::from_hex(raw.lines().next().unwrap()).unwrap())
            .contains("dentist")
    );
    assert_eq!(b.recv(&relay).applied, 1);
    assert_eq!(a.recv(&relay).applied, 1);
    assert_eq!(b.title(&t1), "SYNTHETIC secret dentist appointment");
    assert_eq!(a.title(&t2), "SYNTHETIC buy chalk");
    let outbox_plain: i64 = a
        .conn
        .query_row(
            "SELECT COUNT(*) FROM outbox WHERE ciphertext IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(outbox_plain, 0);
    // Re-pull is a no-op (cursor advanced) and repeat push is deduped by op_id.
    assert_eq!(b.recv(&relay), PullReport::default());
    let seq_before = relay.pull("acct-1", "dev-a", 0).unwrap().len();
    a.conn
        .execute("UPDATE outbox SET status='Sent'", [])
        .unwrap(); // crash after relay accept, before Acked
    s::push(&a.conn, &a.keys, &relay).unwrap();
    assert_eq!(relay.pull("acct-1", "dev-a", 0).unwrap().len(), seq_before);
}

#[test]
fn concurrent_edit_conflict_is_recorded_and_both_converge() {
    let dir = tempfile::tempdir().unwrap();
    let (relay, mut a, mut b) = setup(dir.path());
    let id = a.create("SYNTHETIC draft");
    a.send(&relay);
    b.recv(&relay);
    a.rename(&id, "SYNTHETIC from A", "2026-10-19T10:00:00Z");
    b.rename(&id, "SYNTHETIC from B", "2026-10-19T10:05:00Z");
    a.send(&relay);
    b.send(&relay);
    let ra = a.recv(&relay);
    let rb = b.recv(&relay);
    assert_eq!(ra.conflicts + rb.conflicts, 2);
    assert_eq!(a.title(&id), "SYNTHETIC from B");
    assert_eq!(b.title(&id), "SYNTHETIC from B");
    let losers: String = a
        .conn
        .query_row("SELECT local_json FROM sync_conflicts", [], |r| r.get(0))
        .unwrap();
    assert!(
        losers.contains("SYNTHETIC from A"),
        "losing edit preserved for review"
    );
}

#[test]
fn delete_vs_edit_keeps_edit_on_both_devices() {
    let dir = tempfile::tempdir().unwrap();
    let (relay, mut a, mut b) = setup(dir.path());
    let id = a.create("SYNTHETIC keep me");
    a.send(&relay);
    b.recv(&relay);
    a.delete(&id, "2026-10-19T11:00:00Z");
    b.rename(&id, "SYNTHETIC edited offline", "2026-10-19T10:00:00Z");
    a.send(&relay);
    b.send(&relay);
    a.recv(&relay);
    b.recv(&relay);
    for c in [&a, &b] {
        let del: Option<String> = c
            .conn
            .query_row("SELECT deleted_at FROM tasks WHERE id=?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(del.is_none());
        assert_eq!(c.title(&id), "SYNTHETIC edited offline");
    }
}

#[test]
fn out_of_order_stale_op_does_not_regress() {
    let dir = tempfile::tempdir().unwrap();
    let (relay, mut a, mut b) = setup(dir.path());
    let id = a.create("SYNTHETIC v1");
    a.rename(&id, "SYNTHETIC v2", "2026-10-19T10:00:00Z");
    s::seal_pending(&mut a.conn, &a.keys).unwrap();
    // Snapshot both ops at final state, then push newest first.
    let ids: Vec<String> = a
        .conn
        .prepare("SELECT op_id FROM outbox ORDER BY logical_seq DESC")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    for op in &ids {
        a.conn
            .execute(
                "UPDATE outbox SET status=CASE WHEN op_id=?1 THEN 'Pending' ELSE 'Acked' END",
                [op],
            )
            .unwrap();
        s::push(&a.conn, &a.keys, &relay).unwrap();
    }
    let r = b.recv(&relay);
    assert_eq!(r.applied + r.stale, 2);
    assert_eq!(b.title(&id), "SYNTHETIC v2");
}

#[test]
fn tampered_or_reepoched_ops_are_quarantined_not_applied() {
    let dir = tempfile::tempdir().unwrap();
    let (relay, mut a, mut b) = setup(dir.path());
    a.create("SYNTHETIC one");
    a.create("SYNTHETIC two");
    a.send(&relay);
    let ops = relay.pull("acct-1", "dev-a", 0).unwrap();
    let mut bad = k::from_hex(&ops[0].1.ciphertext).unwrap();
    bad[3] ^= 0x40;
    relay
        .tamper(&ops[0].1.op_id, &k::to_hex(&bad), None)
        .unwrap();
    relay
        .tamper(&ops[1].1.op_id, &ops[1].1.ciphertext, Some(2))
        .unwrap(); // relay re-labels epoch: AAD mismatch
    let r = b.recv(&relay);
    assert_eq!((r.quarantined, r.applied), (2, 0));
    let tasks: i64 = b
        .conn
        .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tasks, 0);
    let audited: i64 = b
        .conn
        .query_row(
            "SELECT COUNT(*) FROM audit_events WHERE actor='Sync' AND outcome='quarantined'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(audited, 2);
}

#[test]
fn revocation_rotates_epoch_and_locks_out_revoked_device() {
    let dir = tempfile::tempdir().unwrap();
    let (relay, mut a, mut b) = setup(dir.path());
    // Third device C, paired by A.
    let offer = s::pair_offer(&a.keys, &relay, "dev-c", "2026-10-19T08:00:00Z").unwrap();
    let (c_keys, confirm) = s::pair_accept(&offer.code, &offer.blob).unwrap();
    s::pair_complete(
        &mut a.keys,
        &a.conn,
        offer,
        &confirm,
        "Laptop",
        "2026-10-19T08:05:00Z",
    )
    .unwrap();
    let mut c = Client {
        conn: open_db(&dir.path().join("c.sqlite")),
        keys: c_keys,
    };
    let rot = s::revoke_device(
        &mut a.keys,
        &relay,
        &a.conn,
        "dev-c",
        "2026-10-19T12:00:00Z",
    )
    .unwrap();
    assert_eq!(rot.new_epoch, 2);
    assert_eq!(rot.wrapped_for, vec!["dev-b".to_string()]);
    assert!(rot.unreachable.is_empty());
    let id = a.create("SYNTHETIC post-revocation");
    a.send(&relay);
    assert_eq!(b.recv(&relay).applied, 1, "B learns epoch 2 from its wrap");
    assert_eq!(b.title(&id), "SYNTHETIC post-revocation");
    assert!(matches!(
        s::pull(&mut c.conn, &mut c.keys, &relay, "x"),
        Err(SyncError::Forbidden)
    ));
    c.create("SYNTHETIC from revoked");
    s::seal_pending(&mut c.conn, &c.keys).unwrap();
    assert!(matches!(
        s::push(&c.conn, &c.keys, &relay),
        Err(SyncError::Forbidden)
    ));
    assert!(matches!(
        relay.register_device("acct-1", "dev-c"),
        Err(SyncError::Forbidden)
    ));
    // The local relay trusts the client-supplied device id (hosted RLS/session
    // binding is unbuilt). Even so, revoked C impersonating dev-b obtains B's wrap
    // but cannot open it: it is sealed to B's X25519 public key, not a shared key.
    let mut spoof = SyncKeys::from_secret(&c.keys.to_secret()).unwrap();
    spoof.device_id = "dev-b".into();
    let mut spoof_conn = open_db(&dir.path().join("spoof.sqlite"));
    let r = s::pull(&mut spoof_conn, &mut spoof, &relay, "x").unwrap();
    assert_eq!(
        (r.applied, r.quarantined),
        (0, 1),
        "epoch-2 op must not open"
    );
    let leaked: i64 = spoof_conn
        .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(leaked, 0);
    // Owner isolation: another account cannot read acct-1 ops.
    relay.register_device("acct-2", "dev-x").unwrap();
    assert!(relay.pull("acct-2", "dev-x", 0).unwrap().is_empty());
    assert!(matches!(
        relay.pull("acct-1", "dev-x", 0),
        Err(SyncError::Forbidden)
    ));
}

#[test]
fn backup_during_wal_writes_and_restore_with_recovery_code() {
    let dir = tempfile::tempdir().unwrap();
    let (_relay, mut a, _b) = setup(dir.path());
    let id = a.create("SYNTHETIC backed up");
    let writer = open_db(&dir.path().join("a.sqlite"));
    writer.execute("BEGIN IMMEDIATE", []).unwrap();
    writer
        .execute("UPDATE tasks SET notes='uncommitted' WHERE id=?1", [&id])
        .unwrap();
    let code = s::new_recovery_code().unwrap();
    let out = dir.path().join("rei.backup");
    s::backup(&a.conn, Some(&a.keys), &code, &out).unwrap();
    writer.execute("COMMIT", []).unwrap();
    assert!(!String::from_utf8_lossy(&std::fs::read(&out).unwrap()).contains("backed up"));
    let dest = dir.path().join("restored.sqlite");
    std::fs::write(&dest, b"existing").unwrap();
    assert!(matches!(
        s::restore(&out, &s::new_recovery_code().unwrap(), &dest),
        Err(SyncError::Crypto(_))
    ));
    assert_eq!(
        std::fs::read(&dest).unwrap(),
        b"existing",
        "wrong code leaves target untouched"
    );
    let keys = s::restore(&out, &code, &dest).unwrap().unwrap();
    assert_eq!(keys.device_id, "dev-a");
    let r = open_db(&dest);
    let (title, notes): (String, String) = r
        .query_row("SELECT title, notes FROM tasks WHERE id=?1", [&id], |x| {
            Ok((x.get(0)?, x.get(1)?))
        })
        .unwrap();
    assert_eq!(
        (title.as_str(), notes.as_str()),
        ("SYNTHETIC backed up", "")
    );
    assert_eq!(db::schema_version(&r).unwrap(), db::supported_version());
    let mut file = std::fs::read(&out).unwrap();
    let last = file.len() - 1;
    file[last] ^= 1;
    std::fs::write(&out, file).unwrap();
    assert!(s::restore(&out, &code, &dest).is_err());
}

#[test]
fn expired_pairing_offer_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let relay = Relay::open(&dir.path().join("relay.sqlite")).unwrap();
    let conn = open_db(&dir.path().join("a.sqlite"));
    let mut a = SyncKeys::create("acct-1", "dev-a").unwrap();
    relay.register_device("acct-1", "dev-a").unwrap();
    let offer = s::pair_offer(&a, &relay, "dev-late", "2026-10-19T08:00:00Z").unwrap();
    let (_late, confirm) = s::pair_accept(&offer.code, &offer.blob).unwrap();
    assert!(s::pair_complete(
        &mut a,
        &conn,
        offer,
        &confirm,
        "Late",
        "2026-10-19T08:10:01Z"
    )
    .is_err());
    assert!(a.peer_ids().is_empty(), "expired confirmation adds no peer");
}
