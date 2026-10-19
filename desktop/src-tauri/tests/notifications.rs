//! P10 notification tests against real SQLite. All records are SYNTHETIC. No OS notification API
//! is called: the sink is a closure standing in for it (OS permission granted/denied simulated).
use rei_os_lib::db;
use rei_os_lib::notifications::*;
use rusqlite::Connection;

const T0: &str = "2026-10-19T08:00:00Z";

fn conn() -> Connection {
    db::open_in_memory().unwrap()
}

fn count(c: &Connection, sql: &str) -> i64 {
    c.query_row(sql, [], |r| r.get(0)).unwrap()
}

fn task(c: &Connection, id: &str, due: &str) {
    c.execute(
        "INSERT INTO tasks(id, created_at, updated_at, origin_device_id, title, due_at_utc, due_tz)
         VALUES (?1, ?2, ?2, 'd', ?1, ?3, 'UTC')",
        rusqlite::params![id, T0, due],
    )
    .unwrap();
}

fn ok_sink(_: &str, _: &str, _: &str) -> Result<(), SinkError> {
    Ok(())
}

#[test]
fn categories_are_opt_in_and_disabled_has_no_effect() {
    let mut c = conn();
    assert!(!category_enabled(&c, "deadline").unwrap());
    task(&c, "t1", "2026-10-19T09:00:00Z");
    assert_eq!(scan_deadlines(&c, T0, 24).unwrap(), 0);
    assert_eq!(morning_brief(&c, "a", "2026-10-19", T0).unwrap(), None);
    assert_eq!(
        enqueue(&c, "k", "inbox", "x", "", T0, T0).unwrap(),
        Enqueue::Disabled
    );
    assert_eq!(count(&c, "SELECT count(*) FROM notification_receipts"), 0);
    assert_eq!(count(&c, "SELECT count(*) FROM briefings"), 0);
    let mut delivered = 0;
    let rep = tick(&mut c, "a", T0, |_, _, _| {
        delivered += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!((rep.delivered, delivered), (0, 0));
    assert!(set_category(&c, "bogus", true, T0).is_err());
}

#[test]
fn dedupe_prevents_repeat_alerts() {
    let mut c = conn();
    set_category(&c, "deadline", true, T0).unwrap();
    task(&c, "t1", "2026-10-19T09:00:00Z");
    assert_eq!(scan_deadlines(&c, T0, 24).unwrap(), 1);
    assert_eq!(scan_deadlines(&c, T0, 24).unwrap(), 0);
    let mut n = 0;
    tick(&mut c, "a", "2026-10-19T09:00:00Z", |_, _, _| {
        n += 1;
        Ok(())
    })
    .unwrap();
    tick(&mut c, "a", "2026-10-19T09:05:00Z", |_, _, _| {
        n += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(n, 1);
    // Rescan after delivery still dedupes on task+due.
    assert_eq!(scan_deadlines(&c, "2026-10-19T09:10:00Z", 24).unwrap(), 0);
    assert_eq!(count(&c, "SELECT count(*) FROM notification_receipts"), 1);
}

#[test]
fn quiet_hours_wrap_midnight_and_defer_until_end() {
    let mut c = conn();
    set_category(&c, "inbox", true, T0).unwrap();
    assert!(set_quiet_hours(&c, true, "22:00", "22:00", "UTC", T0).is_err());
    assert!(set_quiet_hours(&c, true, "9:00", "07:00", "UTC", T0).is_err());
    assert!(set_quiet_hours(&c, true, "22:00", "07:00", "Not/AZone", T0).is_err());
    // 22:00-07:00 in Jakarta (UTC+7) == 15:00-00:00 UTC.
    set_quiet_hours(&c, true, "22:00", "07:00", "Asia/Jakarta", T0).unwrap();
    assert!(is_quiet_now(&c, "2026-10-19T16:00:00Z").unwrap());
    assert!(!is_quiet_now(&c, "2026-10-19T08:00:00Z").unwrap());
    assert!(is_quiet_now(&c, "2026-10-19T23:59:00Z").unwrap());
    enqueue(
        &c,
        "m1",
        "inbox",
        "mail",
        "",
        "2026-10-19T16:00:00Z",
        "2026-10-19T16:00:00Z",
    )
    .unwrap();
    let r = tick(&mut c, "a", "2026-10-19T16:30:00Z", ok_sink).unwrap();
    assert_eq!((r.delivered, r.deferred_quiet), (0, 1));
    let r = tick(&mut c, "a", "2026-10-20T00:30:00Z", ok_sink).unwrap();
    assert_eq!((r.delivered, r.deferred_quiet), (1, 0));
}

#[test]
fn snooze_defers_then_fires_and_dismiss_settles() {
    let mut c = conn();
    set_category(&c, "teams", true, T0).unwrap();
    let Enqueue::Created(id) = enqueue(&c, "s1", "teams", "chat", "", T0, T0).unwrap() else {
        panic!()
    };
    assert!(snooze(&c, &id, T0, T0).is_err());
    snooze(&c, &id, "2026-10-19T08:30:00Z", T0).unwrap();
    let r = tick(&mut c, "a", "2026-10-19T08:10:00Z", ok_sink).unwrap();
    assert_eq!((r.delivered, r.deferred_snooze), (0, 1));
    let r = tick(&mut c, "a", "2026-10-19T08:31:00Z", ok_sink).unwrap();
    assert_eq!(r.delivered, 1);
    let Enqueue::Created(id2) = enqueue(&c, "s2", "teams", "chat2", "", T0, T0).unwrap() else {
        panic!()
    };
    dismiss(&c, &id2, T0).unwrap();
    assert!(dismiss(&c, &id2, T0).is_err());
    let r = tick(&mut c, "a", "2026-10-19T08:40:00Z", ok_sink).unwrap();
    assert_eq!(r.delivered, 0);
}

#[test]
fn two_device_lease_blocks_then_expires_with_epoch_bump() {
    let mut c = conn();
    set_category(&c, "inbox", true, T0).unwrap();
    enqueue(&c, "m", "inbox", "mail", "", T0, T0).unwrap();
    let a = lease_acquire(&mut c, "win", T0, 120).unwrap().unwrap();
    assert_eq!(a.epoch, 1);
    // The Mac cannot deliver while the Windows lease is live.
    let r = tick(&mut c, "mac", "2026-10-19T08:01:00Z", ok_sink).unwrap();
    assert!(!r.lease_held);
    assert_eq!(r.delivered, 0);
    assert_eq!(
        lease_holder(&c, "2026-10-19T08:01:00Z")
            .unwrap()
            .unwrap()
            .holder,
        "win"
    );
    // Windows renews, extending exclusion.
    lease_acquire(&mut c, "win", "2026-10-19T08:01:30Z", 120).unwrap();
    assert!(lease_acquire(&mut c, "mac", "2026-10-19T08:03:00Z", 120)
        .unwrap()
        .is_none());
    // Windows sleeps; the lease expires; the Mac takes over and delivers exactly once.
    assert!(lease_holder(&c, "2026-10-19T08:04:00Z").unwrap().is_none());
    let mut n = 0;
    let r = tick(&mut c, "mac", "2026-10-19T08:04:00Z", |_, _, _| {
        n += 1;
        Ok(())
    })
    .unwrap();
    assert!(r.lease_held);
    assert_eq!(n, 1);
    let l = lease_holder(&c, "2026-10-19T08:05:00Z").unwrap().unwrap();
    assert_eq!((l.holder.as_str(), l.epoch), ("mac", 2));
    let r = tick(&mut c, "win", "2026-10-19T08:05:00Z", ok_sink).unwrap();
    assert!(!r.lease_held);
    assert!(lease_release(&c, "mac").unwrap());
    assert!(!lease_release(&c, "mac").unwrap());
    assert!(lease_acquire(&mut c, "win", "2026-10-19T08:06:00Z", 0).is_err());
    assert_eq!(
        count(&c, "SELECT count(*) FROM notification_receipts WHERE status='Delivered' AND delivered_by_device='mac'"),
        1
    );
}

#[test]
fn sleep_resume_catch_up_collapses_stale_and_reports_gap() {
    let mut c = conn();
    set_category(&c, "inbox", true, T0).unwrap();
    set_category(&c, "deadline", true, T0).unwrap();
    tick(&mut c, "a", T0, ok_sink).unwrap();
    for i in 0..3 {
        let k = format!("m{i}");
        enqueue(
            &c,
            &k,
            "inbox",
            &format!("mail {i}"),
            "",
            "2026-10-19T09:00:00Z",
            T0,
        )
        .unwrap();
    }
    task(&c, "late", "2026-10-19T10:00:00Z");
    scan_deadlines(&c, T0, 24).unwrap();
    // App asleep for 2 days.
    let wake = "2026-10-21T08:00:00Z";
    let mut titles = vec![];
    let r = tick(&mut c, "a", wake, |_, t, _| {
        titles.push(t.to_string());
        Ok(())
    })
    .unwrap();
    assert_eq!(r.gap_minutes, Some(2 * 24 * 60));
    assert_eq!(r.missed, 3);
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM notification_receipts WHERE status='Missed'"
        ),
        3
    );
    // One digest plus the overdue deadline (deadlines are never silently dropped); no mail flood.
    assert_eq!(titles.len(), 2, "{titles:?}");
    assert!(titles
        .iter()
        .any(|t| t.starts_with("3 notifications missed")));
    assert!(titles.iter().any(|t| t == "Due: late"));
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM audit_events WHERE action='catch_up_missed'"
        ),
        1
    );
}

#[test]
fn os_permission_denied_is_recorded_and_retried() {
    let mut c = conn();
    set_category(&c, "inbox", true, T0).unwrap();
    enqueue(&c, "m", "inbox", "mail", "", T0, T0).unwrap();
    let r = tick(&mut c, "a", T0, |_, _, _| Err(SinkError::PermissionDenied)).unwrap();
    assert_eq!((r.delivered, r.permission_denied), (0, 1));
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM notification_receipts WHERE status='PermissionDenied'"
        ),
        1
    );
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM audit_events WHERE outcome='os_permission_denied'"
        ),
        1
    );
    // Permission granted later: the same receipt now delivers.
    let r = tick(&mut c, "a", "2026-10-19T08:10:00Z", ok_sink).unwrap();
    assert_eq!(r.delivered, 1);
    // Other sink errors leave it pending for retry.
    enqueue(&c, "m2", "inbox", "mail2", "", T0, T0).unwrap();
    tick(&mut c, "a", "2026-10-19T08:11:00Z", |_, _, _| {
        Err(SinkError::Other("x".into()))
    })
    .unwrap();
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM notification_receipts WHERE status='Pending'"
        ),
        1
    );
}

#[test]
fn morning_brief_is_local_once_per_date_and_disclosure_is_honest() {
    let mut c = conn();
    let d = disclosure(&c, T0).unwrap();
    assert!(d.text.contains("never run"));
    set_category(&c, "morning_brief", true, T0).unwrap();
    task(&c, "Submit report", "2026-10-19T12:00:00Z");
    task(&c, "Tomorrow thing", "2026-10-20T12:00:00Z");
    let b = morning_brief(&c, "win", "2026-10-19", T0).unwrap().unwrap();
    assert!(b.contains("Submit report") && !b.contains("Tomorrow thing"));
    // The second device on the same date is a no-op.
    assert_eq!(morning_brief(&c, "mac", "2026-10-19", T0).unwrap(), None);
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM notification_receipts WHERE category='morning_brief'"
        ),
        1
    );
    assert!(morning_brief(&c, "win", "not-a-date", T0).is_err());
    tick(&mut c, "win", T0, ok_sink).unwrap();
    lease_release(&c, "win").unwrap();
    let d = disclosure(&c, "2026-10-19T09:00:00Z").unwrap();
    assert!(d.lease_holder.is_none() && d.text.contains("stop while all devices are off"));
    // Opting out after enqueue stops delivery.
    enqueue(&c, "x", "morning_brief", "late", "", T0, T0).unwrap();
    set_category(&c, "morning_brief", false, T0).unwrap();
    let r = tick(&mut c, "win", "2026-10-19T09:00:00Z", ok_sink).unwrap();
    assert_eq!(r.delivered, 0);
}
