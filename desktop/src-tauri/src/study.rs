//! P07 university/study: courses (HM1 etc.), assignments/exams/notes, study logs,
//! language logs, HTB progress bookkeeping, LaTeX text export and Anki file interop.
//! No TeX compiler, AnkiConnect or offensive tool is ever invoked.
use crate::files;
use crate::planner::{Ctx, Res};
use crate::records::{self as rec, val};
use rusqlite::types::Value;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

fn opt_date(d: &Option<String>, field: &str) -> Res<Value> {
    Ok(match d {
        Some(d) => Value::from(rec::date(d, field)?),
        None => Value::Null,
    })
}

// ------------------------------------------------------------------ courses

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CourseReq {
    pub name: String,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub term: String,
}

pub fn course_create(conn: &mut Connection, ctx: &Ctx, r: CourseReq) -> Res<Json> {
    let cols = vec![
        ("name", rec::text(&r.name, "name", 200)?.into()),
        ("code", rec::free_text(&r.code, "code", 60)?.into()),
        ("term", rec::free_text(&r.term, "term", 60)?.into()),
    ];
    let tx = conn.transaction()?;
    let id = rec::insert(&tx, ctx, "courses", "course", cols)?;
    tx.commit()?;
    rec::get(conn, "courses", &id)
}

pub fn course_list(conn: &Connection) -> Res<Vec<Json>> {
    rec::list(conn, "courses", "", vec![], "name")
}

pub fn course_update(
    conn: &mut Connection,
    ctx: &Ctx,
    id: &str,
    expected: u64,
    r: CourseReq,
) -> Res<Json> {
    let cols = vec![
        ("name", rec::text(&r.name, "name", 200)?.into()),
        ("code", rec::free_text(&r.code, "code", 60)?.into()),
        ("term", rec::free_text(&r.term, "term", 60)?.into()),
    ];
    let tx = conn.transaction()?;
    rec::update(&tx, ctx, "courses", "course", id, expected, cols)?;
    tx.commit()?;
    rec::get(conn, "courses", id)
}

pub fn course_delete(
    conn: &mut Connection,
    ctx: &Ctx,
    id: &str,
    exp: u64,
    confirm: bool,
) -> Res<()> {
    rec::soft_delete(conn, ctx, "courses", "course", id, exp, confirm)
}

// ------------------------------------------------------------------ items

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StudyItemReq {
    pub course_id: String,
    pub kind: String,
    pub title: String,
    pub due_date: Option<String>,
    #[serde(default)]
    pub body: String,
}

fn item_cols(r: &StudyItemReq) -> Res<Vec<(&'static str, Value)>> {
    Ok(vec![
        ("course_id", r.course_id.clone().into()),
        (
            "kind",
            rec::one_of(&r.kind, "kind", &["assignment", "exam", "homework", "note"])?.into(),
        ),
        ("title", rec::text(&r.title, "title", 500)?.into()),
        ("due_date", opt_date(&r.due_date, "due_date")?),
        ("body", rec::free_text(&r.body, "body", 65_536)?.into()),
    ])
}

pub fn study_item_create(conn: &mut Connection, ctx: &Ctx, r: StudyItemReq) -> Res<Json> {
    rec::exists(conn, "courses", &r.course_id)?;
    let cols = item_cols(&r)?;
    let tx = conn.transaction()?;
    let id = rec::insert(&tx, ctx, "study_items", "study_item", cols)?;
    tx.commit()?;
    rec::get(conn, "study_items", &id)
}

pub fn study_item_update(
    conn: &mut Connection,
    ctx: &Ctx,
    id: &str,
    expected: u64,
    status: Option<&str>,
    r: StudyItemReq,
) -> Res<Json> {
    rec::exists(conn, "courses", &r.course_id)?;
    let mut cols = item_cols(&r)?;
    if let Some(s) = status {
        cols.push((
            "status",
            rec::one_of(s, "status", &["open", "done", "cancelled"])?.into(),
        ));
    }
    let tx = conn.transaction()?;
    rec::update(&tx, ctx, "study_items", "study_item", id, expected, cols)?;
    tx.commit()?;
    rec::get(conn, "study_items", id)
}

/// Items for one course, or all; only items with an explicit due date sort first.
pub fn study_item_list(conn: &Connection, course_id: Option<&str>) -> Res<Vec<Json>> {
    let order = "due_date IS NULL, due_date, created_at";
    match course_id {
        Some(c) => rec::list(
            conn,
            "study_items",
            "AND course_id=?1",
            vec![c.to_string().into()],
            order,
        ),
        None => rec::list(conn, "study_items", "", vec![], order),
    }
}

pub fn study_item_delete(
    conn: &mut Connection,
    ctx: &Ctx,
    id: &str,
    exp: u64,
    confirm: bool,
) -> Res<()> {
    rec::soft_delete(conn, ctx, "study_items", "study_item", id, exp, confirm)
}

// ------------------------------------------------------------------ logs

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StudySessionReq {
    pub course_id: Option<String>,
    pub topic: String,
    pub minutes: i64,
    pub day: String,
}

pub fn study_session_create(conn: &mut Connection, ctx: &Ctx, r: StudySessionReq) -> Res<Json> {
    if let Some(c) = &r.course_id {
        rec::exists(conn, "courses", c)?;
    }
    if !(1..=1440).contains(&r.minutes) {
        return Err(val("minutes must be 1-1440"));
    }
    let cols = vec![
        (
            "course_id",
            r.course_id.clone().map_or(Value::Null, Value::from),
        ),
        ("topic", rec::text(&r.topic, "topic", 500)?.into()),
        ("minutes", r.minutes.into()),
        ("day", rec::date(&r.day, "day")?.into()),
    ];
    let tx = conn.transaction()?;
    let id = rec::insert(&tx, ctx, "study_sessions", "study_session", cols)?;
    tx.commit()?;
    rec::get(conn, "study_sessions", &id)
}

pub fn study_session_list(conn: &Connection) -> Res<Vec<Json>> {
    rec::list(
        conn,
        "study_sessions",
        "",
        vec![],
        "day DESC, created_at DESC",
    )
}

/// Total logged study minutes per course (None = unassigned), integer arithmetic only.
pub fn study_minutes_by_course(conn: &Connection) -> Res<Vec<(Option<String>, i64)>> {
    let mut st = conn.prepare(
        "SELECT course_id, SUM(minutes) FROM study_sessions WHERE deleted_at IS NULL
         GROUP BY course_id ORDER BY course_id",
    )?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageLogReq {
    pub language: String,
    pub activity: String,
    pub minutes: i64,
    pub day: String,
    #[serde(default)]
    pub notes: String,
}

pub fn language_log_create(conn: &mut Connection, ctx: &Ctx, r: LanguageLogReq) -> Res<Json> {
    if !(1..=1440).contains(&r.minutes) {
        return Err(val("minutes must be 1-1440"));
    }
    let cols = vec![
        ("language", rec::text(&r.language, "language", 60)?.into()),
        ("activity", rec::text(&r.activity, "activity", 200)?.into()),
        ("minutes", r.minutes.into()),
        ("day", rec::date(&r.day, "day")?.into()),
        ("notes", rec::free_text(&r.notes, "notes", 8192)?.into()),
    ];
    let tx = conn.transaction()?;
    let id = rec::insert(&tx, ctx, "language_logs", "language_log", cols)?;
    tx.commit()?;
    rec::get(conn, "language_logs", &id)
}

pub fn language_log_list(conn: &Connection) -> Res<Vec<Json>> {
    rec::list(
        conn,
        "language_logs",
        "",
        vec![],
        "day DESC, created_at DESC",
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HtbReq {
    pub name: String,
    pub kind: String,
    pub status: String,
    #[serde(default)]
    pub notes: String,
}

fn htb_cols(r: &HtbReq) -> Res<Vec<(&'static str, Value)>> {
    Ok(vec![
        ("name", rec::text(&r.name, "name", 200)?.into()),
        (
            "kind",
            rec::one_of(
                &r.kind,
                "kind",
                &["machine", "challenge", "module", "track"],
            )?
            .into(),
        ),
        (
            "status",
            rec::one_of(
                &r.status,
                "status",
                &["planned", "in_progress", "completed"],
            )?
            .into(),
        ),
        ("notes", rec::free_text(&r.notes, "notes", 8192)?.into()),
    ])
}

pub fn htb_create(conn: &mut Connection, ctx: &Ctx, r: HtbReq) -> Res<Json> {
    let cols = htb_cols(&r)?;
    let tx = conn.transaction()?;
    let id = rec::insert(&tx, ctx, "htb_progress", "htb_progress", cols)?;
    tx.commit()?;
    rec::get(conn, "htb_progress", &id)
}

pub fn htb_update(
    conn: &mut Connection,
    ctx: &Ctx,
    id: &str,
    expected: u64,
    r: HtbReq,
) -> Res<Json> {
    let cols = htb_cols(&r)?;
    let tx = conn.transaction()?;
    rec::update(&tx, ctx, "htb_progress", "htb_progress", id, expected, cols)?;
    tx.commit()?;
    rec::get(conn, "htb_progress", id)
}

pub fn htb_list(conn: &Connection) -> Res<Vec<Json>> {
    rec::list(conn, "htb_progress", "", vec![], "status, name")
}

// ------------------------------------------------------------------ LaTeX

/// Escapes text so no TeX control sequence in the source can take effect: every backslash
/// becomes a literal, braces/specials are escaped, control characters are dropped.
pub fn latex_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\textbackslash{}"),
            '{' => o.push_str("\\{"),
            '}' => o.push_str("\\}"),
            '$' => o.push_str("\\$"),
            '&' => o.push_str("\\&"),
            '#' => o.push_str("\\#"),
            '_' => o.push_str("\\_"),
            '%' => o.push_str("\\%"),
            '^' => o.push_str("\\textasciicircum{}"),
            '~' => o.push_str("\\textasciitilde{}"),
            '\n' => o.push('\n'),
            c if c.is_control() => {}
            c => o.push(c),
        }
    }
    o
}

/// Builds a standalone `.tex` document as text. This function never runs a compiler.
pub fn latex_export(conn: &Connection, course_id: Option<&str>) -> Res<String> {
    let courses: Vec<Json> = match course_id {
        Some(c) => vec![rec::get(conn, "courses", c)?],
        None => course_list(conn)?,
    };
    let mut out = String::from(
        "\\documentclass{article}\n\\begin{document}\n\\title{Study notes}\n\\maketitle\n",
    );
    for c in courses {
        let name = c["name"].as_str().unwrap_or_default();
        out.push_str(&format!("\\section*{{{}}}\n", latex_escape(name)));
        let items = study_item_list(conn, c["id"].as_str())?;
        if items.is_empty() {
            out.push_str("No entries.\n");
        }
        for i in items {
            let due = i["due_date"]
                .as_str()
                .map(|d| format!(" (due {})", latex_escape(d)))
                .unwrap_or_default();
            out.push_str(&format!(
                "\\subsection*{{{}: {}{}}}\n{}\n\n",
                latex_escape(i["kind"].as_str().unwrap_or("")),
                latex_escape(i["title"].as_str().unwrap_or("")),
                due,
                latex_escape(i["body"].as_str().unwrap_or(""))
            ));
        }
    }
    out.push_str("\\end{document}\n");
    Ok(out)
}

// ------------------------------------------------------------------ Anki

fn anki_field(s: &str) -> String {
    s.replace('\t', "    ").replace(['\r', '\n'], " ")
}

/// Anki "Import File" text format (tab separated, plain text, deck/tags columns).
pub fn anki_export_text(conn: &Connection) -> Res<String> {
    let mut out = String::from("#separator:tab\n#html:false\n#deck column:1\n#tags column:4\n");
    for c in rec::list(conn, "anki_cards", "", vec![], "deck, created_at")? {
        let f = |k: &str| anki_field(c[k].as_str().unwrap_or(""));
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            f("deck"),
            f("front"),
            f("back"),
            f("tags")
        ));
    }
    Ok(out)
}

#[derive(Debug, Serialize, PartialEq)]
pub struct AnkiImportReport {
    pub imported: u32,
    pub duplicates: u32,
    pub skipped: u32,
}

/// Imports an Anki text export. Header directives are honoured only for tab separation
/// and deck/tags columns; anything else is rejected rather than guessed. Content is stored
/// as inert text.
pub fn anki_import_text(
    conn: &mut Connection,
    ctx: &Ctx,
    content: &str,
    default_deck: &str,
) -> Res<AnkiImportReport> {
    if content.len() as u64 > files::MAX_IMPORT_BYTES {
        return Err(val("import exceeds the 5 MiB limit"));
    }
    let default_deck = rec::text(default_deck, "deck", 200)?;
    let (mut deck_col, mut tags_col) = (None, None);
    let mut rows: Vec<Vec<&str>> = vec![];
    let mut report = AnkiImportReport {
        imported: 0,
        duplicates: 0,
        skipped: 0,
    };
    for line in content.lines().map(|l| l.trim_end_matches('\r')) {
        if let Some(d) = line.strip_prefix('#') {
            let (k, v) = d.split_once(':').unwrap_or((d, ""));
            match k {
                "separator" if v.trim() != "tab" && v.trim() != "Tab" => {
                    return Err(val("only tab-separated Anki exports are supported"))
                }
                "html" if v.trim() == "true" => {
                    return Err(val(
                        "html Anki exports are not imported; export as plain text",
                    ))
                }
                "deck column" => deck_col = v.trim().parse::<usize>().ok().filter(|n| *n > 0),
                "tags column" => tags_col = v.trim().parse::<usize>().ok().filter(|n| *n > 0),
                _ => {}
            }
        } else if !line.trim().is_empty() {
            rows.push(line.split('\t').collect());
        }
    }
    if rows.len() > 10_000 {
        return Err(val("import exceeds 10000 cards"));
    }
    let tx = conn.transaction()?;
    for f in rows {
        let get = |col: Option<usize>| col.and_then(|c| f.get(c - 1).copied());
        let rest: Vec<&str> = f
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(i + 1) != deck_col && Some(i + 1) != tags_col)
            .map(|(_, v)| *v)
            .collect();
        let (front, back) = (
            rest.first().map(|s| s.trim()),
            rest.get(1).copied().unwrap_or(""),
        );
        let deck = get(deck_col)
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .unwrap_or(&default_deck);
        let tags = get(tags_col).unwrap_or("");
        let valid = front.is_some_and(|s| !s.is_empty() && s.len() <= 8192)
            && back.len() <= 8192
            && deck.chars().count() <= 200
            && rec::free_text(tags, "tags", 1024).is_ok();
        if !valid {
            report.skipped += 1;
            continue;
        }
        let front = front.unwrap_or_default();
        let dup: i64 = tx.query_row(
            "SELECT COUNT(*) FROM anki_cards WHERE deck=?1 AND front=?2 AND deleted_at IS NULL",
            params![deck, front],
            |r| r.get(0),
        )?;
        if dup > 0 {
            report.duplicates += 1;
            continue;
        }
        rec::insert(
            &tx,
            ctx,
            "anki_cards",
            "anki_card",
            vec![
                ("deck", deck.to_string().into()),
                ("front", front.to_string().into()),
                ("back", back.to_string().into()),
                ("tags", tags.to_string().into()),
            ],
        )?;
        report.imported += 1;
    }
    tx.commit()?;
    Ok(report)
}

/// `anki_import(file_grant_id)` / `anki_export(file_grant_id)` over dialog-granted files.
pub fn anki_import(
    conn: &mut Connection,
    ctx: &Ctx,
    grant_id: &str,
    deck: &str,
) -> Res<AnkiImportReport> {
    let text = files::read_granted(conn, grant_id)?;
    anki_import_text(conn, ctx, &text, deck)
}

pub fn anki_export(conn: &Connection, grant_id: &str) -> Res<()> {
    files::write_granted(conn, grant_id, &anki_export_text(conn)?)
}

pub fn latex_export_to(conn: &Connection, course_id: Option<&str>, grant_id: &str) -> Res<()> {
    files::write_granted(conn, grant_id, &latex_export(conn, course_id)?)
}
