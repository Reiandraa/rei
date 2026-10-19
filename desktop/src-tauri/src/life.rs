//! P08 life/admin: expenses, budgets, income, shifts and admin deadlines. Money is integer
//! minor units with an ISO 4217 currency; currencies are never mixed or converted. There is
//! no payment API and no legal advice: deadlines exist only with a user/source date.
use crate::files;
use crate::planner::{Ctx, Res};
use crate::records::{self as rec, val};
use rusqlite::types::Value;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::collections::BTreeMap;

fn amount(v: i64, field: &str) -> Res<Value> {
    if v <= 0 || v > 1_000_000_000_000 {
        return Err(val(format!(
            "{field} must be a positive amount in minor units"
        )));
    }
    Ok(v.into())
}

/// A document link must be a live 'document' grant; the original file stays where the user keeps it.
fn doc(conn: &Connection, g: &Option<String>) -> Res<Value> {
    match g {
        Some(id) => {
            files::grant_resolve(conn, id, &["document"])?;
            Ok(id.clone().into())
        }
        None => Ok(Value::Null),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpenseReq {
    pub amount_minor: i64,
    pub currency: String,
    pub category: String,
    pub spent_on: String,
    #[serde(default)]
    pub note: String,
    pub document_grant_id: Option<String>,
}

fn expense_cols(conn: &Connection, r: &ExpenseReq) -> Res<Vec<(&'static str, Value)>> {
    Ok(vec![
        ("amount_minor", amount(r.amount_minor, "amount_minor")?),
        ("currency", rec::currency(&r.currency)?.into()),
        ("category", rec::text(&r.category, "category", 100)?.into()),
        ("spent_on", rec::date(&r.spent_on, "spent_on")?.into()),
        ("note", rec::free_text(&r.note, "note", 2048)?.into()),
        ("document_grant_id", doc(conn, &r.document_grant_id)?),
    ])
}

pub fn expense_create(conn: &mut Connection, ctx: &Ctx, r: ExpenseReq) -> Res<Json> {
    let cols = expense_cols(conn, &r)?;
    let tx = conn.transaction()?;
    let id = rec::insert(&tx, ctx, "expenses", "expense", cols)?;
    tx.commit()?;
    rec::get(conn, "expenses", &id)
}

pub fn expense_update(
    conn: &mut Connection,
    ctx: &Ctx,
    id: &str,
    exp: u64,
    r: ExpenseReq,
) -> Res<Json> {
    let cols = expense_cols(conn, &r)?;
    let tx = conn.transaction()?;
    rec::update(&tx, ctx, "expenses", "expense", id, exp, cols)?;
    tx.commit()?;
    rec::get(conn, "expenses", id)
}

pub fn expense_delete(
    conn: &mut Connection,
    ctx: &Ctx,
    id: &str,
    exp: u64,
    confirm: bool,
) -> Res<()> {
    rec::soft_delete(conn, ctx, "expenses", "expense", id, exp, confirm)
}

pub fn expense_list(conn: &Connection, month: Option<&str>) -> Res<Vec<Json>> {
    match month {
        Some(m) => rec::list(
            conn,
            "expenses",
            "AND substr(spent_on,1,7)=?1",
            vec![rec::month(m)?.into()],
            "spent_on DESC, created_at DESC",
        ),
        None => rec::list(
            conn,
            "expenses",
            "",
            vec![],
            "spent_on DESC, created_at DESC",
        ),
    }
}

/// Spreadsheet-safe export: cells starting with = + - @ tab or CR are prefixed with `'`.
pub fn csv_cell(s: &str) -> String {
    let safe = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{s}")
    } else {
        s.to_string()
    };
    if safe.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", safe.replace('"', "\"\""))
    } else {
        safe
    }
}

pub fn expense_csv(conn: &Connection) -> Res<String> {
    let mut out = String::from("date,currency,amount,category,note\n");
    for e in expense_list(conn, None)? {
        let minor = e["amount_minor"].as_i64().unwrap_or(0);
        out.push_str(&format!(
            "{},{},{},{},{}\n",
            csv_cell(e["spent_on"].as_str().unwrap_or("")),
            csv_cell(e["currency"].as_str().unwrap_or("")),
            format_minor(minor),
            csv_cell(e["category"].as_str().unwrap_or("")),
            csv_cell(e["note"].as_str().unwrap_or(""))
        ));
    }
    Ok(out)
}

/// Exact decimal text of a minor amount (2 fraction digits); no floats involved.
pub fn format_minor(m: i64) -> String {
    let sign = if m < 0 { "-" } else { "" };
    let a = m.unsigned_abs();
    format!("{sign}{}.{:02}", a / 100, a % 100)
}

// ------------------------------------------------------------------ budgets

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetReq {
    pub category: String,
    pub currency: String,
    pub month: String,
    pub limit_minor: i64,
}

/// Sets the limit for (category, currency, month): creates it, or updates the existing row.
pub fn budget_set(conn: &mut Connection, ctx: &Ctx, r: BudgetReq) -> Res<Json> {
    if !(0..=1_000_000_000_000).contains(&r.limit_minor) {
        return Err(val(
            "limit_minor must be a non-negative amount in minor units",
        ));
    }
    let (cat, cur, mon) = (
        rec::text(&r.category, "category", 100)?,
        rec::currency(&r.currency)?,
        rec::month(&r.month)?,
    );
    let tx = conn.transaction()?;
    let existing: Option<(String, i64)> = tx
        .query_row(
            "SELECT id, revision FROM budgets WHERE category=?1 AND currency=?2 AND month=?3 AND deleted_at IS NULL",
            params![cat, cur, mon],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let id = match existing {
        Some((id, rev)) => {
            rec::update(
                &tx,
                ctx,
                "budgets",
                "budget",
                &id,
                rev as u64,
                vec![("limit_minor", r.limit_minor.into())],
            )?;
            id
        }
        None => rec::insert(
            &tx,
            ctx,
            "budgets",
            "budget",
            vec![
                ("category", cat.into()),
                ("currency", cur.into()),
                ("month", mon.into()),
                ("limit_minor", r.limit_minor.into()),
            ],
        )?,
    };
    tx.commit()?;
    rec::get(conn, "budgets", &id)
}

#[derive(Debug, Serialize, PartialEq)]
pub struct BudgetLine {
    pub category: String,
    pub currency: String,
    pub limit_minor: i64,
    pub spent_minor: i64,
    pub remaining_minor: i64,
    pub over: bool,
}

pub fn budget_status(conn: &Connection, month: &str) -> Res<Vec<BudgetLine>> {
    let month = rec::month(month)?;
    let mut st = conn.prepare(
        "SELECT b.category, b.currency, b.limit_minor,
                COALESCE((SELECT SUM(e.amount_minor) FROM expenses e
                          WHERE e.deleted_at IS NULL AND e.category=b.category
                            AND e.currency=b.currency AND substr(e.spent_on,1,7)=b.month), 0)
         FROM budgets b WHERE b.deleted_at IS NULL AND b.month=?1 ORDER BY b.currency, b.category",
    )?;
    let rows = st.query_map([month], |r| {
        let (limit, spent): (i64, i64) = (r.get(2)?, r.get(3)?);
        Ok(BudgetLine {
            category: r.get(0)?,
            currency: r.get(1)?,
            limit_minor: limit,
            spent_minor: spent,
            remaining_minor: limit - spent,
            over: spent > limit,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

// ------------------------------------------------------------------ income + shifts

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncomeReq {
    pub amount_minor: i64,
    pub currency: String,
    pub source: String,
    pub received_on: String,
}

pub fn income_create(conn: &mut Connection, ctx: &Ctx, r: IncomeReq) -> Res<Json> {
    let cols = vec![
        ("amount_minor", amount(r.amount_minor, "amount_minor")?),
        ("currency", rec::currency(&r.currency)?.into()),
        ("source", rec::text(&r.source, "source", 200)?.into()),
        (
            "received_on",
            rec::date(&r.received_on, "received_on")?.into(),
        ),
    ];
    let tx = conn.transaction()?;
    let id = rec::insert(&tx, ctx, "income", "income", cols)?;
    tx.commit()?;
    rec::get(conn, "income", &id)
}

pub fn income_list(conn: &Connection) -> Res<Vec<Json>> {
    rec::list(
        conn,
        "income",
        "",
        vec![],
        "received_on DESC, created_at DESC",
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShiftReq {
    pub employer: String,
    pub shift_date: String,
    pub worked_min: i64,
    #[serde(default)]
    pub break_min: i64,
    pub rate_minor_per_hour: i64,
    pub currency: String,
}

/// Gross pay in minor units for paid minutes, rounded half up, using integer math only.
pub fn shift_gross_minor(worked_min: i64, break_min: i64, rate_minor_per_hour: i64) -> Res<i64> {
    let paid = worked_min - break_min;
    if paid < 0 {
        return Err(val("break exceeds worked time"));
    }
    paid.checked_mul(rate_minor_per_hour)
        .map(|n| (n + 30) / 60)
        .ok_or_else(|| val("amount overflow"))
}

pub fn shift_create(conn: &mut Connection, ctx: &Ctx, r: ShiftReq) -> Res<Json> {
    if !(1..=1440).contains(&r.worked_min) || r.break_min < 0 || r.rate_minor_per_hour < 0 {
        return Err(val(
            "worked_min 1-1440, break_min and rate must not be negative",
        ));
    }
    shift_gross_minor(r.worked_min, r.break_min, r.rate_minor_per_hour)?;
    let cols = vec![
        ("employer", rec::text(&r.employer, "employer", 200)?.into()),
        ("shift_date", rec::date(&r.shift_date, "shift_date")?.into()),
        ("worked_min", r.worked_min.into()),
        ("break_min", r.break_min.into()),
        ("rate_minor_per_hour", r.rate_minor_per_hour.into()),
        ("currency", rec::currency(&r.currency)?.into()),
    ];
    let tx = conn.transaction()?;
    let id = rec::insert(&tx, ctx, "shifts", "shift", cols)?;
    tx.commit()?;
    rec::get(conn, "shifts", &id)
}

pub fn shift_list(conn: &Connection) -> Res<Vec<Json>> {
    rec::list(
        conn,
        "shifts",
        "",
        vec![],
        "shift_date DESC, created_at DESC",
    )
}

#[derive(Debug, Serialize, Default, PartialEq)]
pub struct MonthTotals {
    pub income_minor: i64,
    pub shift_gross_minor: i64,
    pub expense_minor: i64,
    pub net_minor: i64,
}

/// Per-currency month summary; different currencies are reported separately, never added.
pub fn month_summary(conn: &Connection, month: &str) -> Res<BTreeMap<String, MonthTotals>> {
    let month = rec::month(month)?;
    let mut out: BTreeMap<String, MonthTotals> = BTreeMap::new();
    let mut add = |cur: String, f: &dyn Fn(&mut MonthTotals)| f(out.entry(cur).or_default());
    for i in rec::list(
        conn,
        "income",
        "AND substr(received_on,1,7)=?1",
        vec![month.clone().into()],
        "id",
    )? {
        let (c, a) = (
            i["currency"].as_str().unwrap_or("").to_string(),
            i["amount_minor"].as_i64().unwrap_or(0),
        );
        add(c, &|t| t.income_minor += a);
    }
    for s in rec::list(
        conn,
        "shifts",
        "AND substr(shift_date,1,7)=?1",
        vec![month.clone().into()],
        "id",
    )? {
        let g = shift_gross_minor(
            s["worked_min"].as_i64().unwrap_or(0),
            s["break_min"].as_i64().unwrap_or(0),
            s["rate_minor_per_hour"].as_i64().unwrap_or(0),
        )?;
        add(s["currency"].as_str().unwrap_or("").to_string(), &|t| {
            t.shift_gross_minor += g
        });
    }
    for e in expense_list(conn, Some(&month))? {
        let a = e["amount_minor"].as_i64().unwrap_or(0);
        add(e["currency"].as_str().unwrap_or("").to_string(), &|t| {
            t.expense_minor += a
        });
    }
    for t in out.values_mut() {
        t.net_minor = t.income_minor + t.shift_gross_minor - t.expense_minor;
    }
    Ok(out)
}

// ------------------------------------------------------------------ admin deadlines

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCaseReq {
    pub title: String,
    pub kind: String,
    pub due_date: Option<String>,
    /// Required with a due date: 'user' (typed in) or 'source' (quoted from a document/message).
    pub date_source: Option<String>,
    #[serde(default)]
    pub source_ref: String,
    #[serde(default)]
    pub notes: String,
    pub document_grant_id: Option<String>,
}

fn admin_cols(conn: &Connection, r: &AdminCaseReq) -> Res<Vec<(&'static str, Value)>> {
    let (due, src) = match (&r.due_date, &r.date_source) {
        (Some(d), Some(s)) => (
            Value::from(rec::date(d, "due_date")?),
            Value::from(rec::one_of(s, "date_source", &["user", "source"])?),
        ),
        (None, None) => (Value::Null, Value::Null),
        _ => {
            return Err(val(
                "a deadline needs both due_date and date_source; none is ever guessed",
            ))
        }
    };
    Ok(vec![
        ("title", rec::text(&r.title, "title", 500)?.into()),
        (
            "kind",
            rec::one_of(
                &r.kind,
                "kind",
                &[
                    "rent",
                    "fintiba",
                    "immigration",
                    "insurance",
                    "rundfunkbeitrag",
                    "other",
                ],
            )?
            .into(),
        ),
        ("due_date", due),
        ("date_source", src),
        (
            "source_ref",
            rec::free_text(&r.source_ref, "source_ref", 1024)?.into(),
        ),
        ("notes", rec::free_text(&r.notes, "notes", 65_536)?.into()),
        ("document_grant_id", doc(conn, &r.document_grant_id)?),
    ])
}

pub fn admin_case_create(conn: &mut Connection, ctx: &Ctx, r: AdminCaseReq) -> Res<Json> {
    let cols = admin_cols(conn, &r)?;
    let tx = conn.transaction()?;
    let id = rec::insert(&tx, ctx, "admin_cases", "admin_case", cols)?;
    tx.commit()?;
    rec::get(conn, "admin_cases", &id)
}

pub fn admin_case_update(
    conn: &mut Connection,
    ctx: &Ctx,
    id: &str,
    exp: u64,
    status: Option<&str>,
    r: AdminCaseReq,
) -> Res<Json> {
    let mut cols = admin_cols(conn, &r)?;
    if let Some(s) = status {
        cols.push((
            "status",
            rec::one_of(s, "status", &["open", "done", "cancelled"])?.into(),
        ));
    }
    let tx = conn.transaction()?;
    rec::update(&tx, ctx, "admin_cases", "admin_case", id, exp, cols)?;
    tx.commit()?;
    rec::get(conn, "admin_cases", id)
}

pub fn admin_case_list(conn: &Connection) -> Res<Vec<Json>> {
    rec::list(
        conn,
        "admin_cases",
        "",
        vec![],
        "status, due_date IS NULL, due_date",
    )
}

pub fn admin_case_delete(
    conn: &mut Connection,
    ctx: &Ctx,
    id: &str,
    exp: u64,
    confirm: bool,
) -> Res<()> {
    rec::soft_delete(conn, ctx, "admin_cases", "admin_case", id, exp, confirm)
}

/// Open cases whose explicit date falls on/before `today + within_days`; undated cases never alert.
pub fn admin_due_soon(conn: &Connection, today: &str, within_days: i64) -> Res<Vec<Json>> {
    let t: jiff::civil::Date = rec::date(today, "today")?
        .parse()
        .map_err(|_| val("bad date"))?;
    let limit = t
        .checked_add(jiff::Span::new().days(within_days.clamp(0, 3650)))
        .map_err(|_| val("date out of range"))?;
    rec::list(
        conn,
        "admin_cases",
        "AND status='open' AND due_date IS NOT NULL AND due_date<=?1",
        vec![limit.to_string().into()],
        "due_date",
    )
}
