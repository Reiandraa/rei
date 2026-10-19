//! File grants and links (docs/THREAT-MODEL.md T14). Only a path the user picked in the OS
//! dialog becomes a `file_grants` row; Rust canonicalizes it at registration and again on
//! every use. Nothing here scans directories or indexes file content.
use crate::commands::IpcError;
use crate::planner::{Ctx, Res};
use crate::records::{self as rec, val};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::Value as Json;
use std::path::{Path, PathBuf};

pub const MAX_IMPORT_BYTES: u64 = 5 * 1024 * 1024;

fn canonical(path: &str, must_exist: bool) -> Res<PathBuf> {
    if path.is_empty() || path.contains('\0') {
        return Err(val("invalid path"));
    }
    let p = Path::new(path);
    if must_exist {
        let c = std::fs::canonicalize(p).map_err(|_| val("selected file does not exist"))?;
        if !c.is_file() {
            return Err(val("selection is not a regular file"));
        }
        return Ok(c);
    }
    // Export target: the file may not exist yet, its directory must.
    let name = p.file_name().ok_or_else(|| val("missing file name"))?;
    let dir = p.parent().filter(|d| !d.as_os_str().is_empty());
    let dir = std::fs::canonicalize(dir.unwrap_or_else(|| Path::new(".")))
        .map_err(|_| val("export directory does not exist"))?;
    let full = dir.join(name);
    if full.is_dir() {
        return Err(val("export target is a directory"));
    }
    Ok(full)
}

/// Registers a dialog-selected path. The renderer never supplies a path for later use,
/// only the returned grant id.
pub fn grant_register(conn: &mut Connection, ctx: &Ctx, path: &str, purpose: &str) -> Res<Json> {
    let purpose = rec::one_of(
        purpose,
        "purpose",
        &["document", "link", "import", "export"],
    )?;
    let c = canonical(path, purpose != "export")?;
    let cs = c.to_string_lossy().to_string();
    let tx = conn.transaction()?;
    let prior: Option<String> = tx
        .query_row(
            "SELECT id FROM file_grants WHERE path=?1 AND purpose=?2 AND revoked_at IS NULL AND deleted_at IS NULL",
            params![cs, purpose],
            |r| r.get(0),
        )
        .optional()?;
    let id = match prior {
        Some(id) => id,
        None => rec::insert(
            &tx,
            ctx,
            "file_grants",
            "file_grant",
            vec![("path", cs.into()), ("purpose", purpose.into())],
        )?,
    };
    tx.commit()?;
    rec::get(conn, "file_grants", &id)
}

pub fn grant_revoke(conn: &mut Connection, ctx: &Ctx, id: &str, expected: u64) -> Res<()> {
    let tx = conn.transaction()?;
    rec::update(
        &tx,
        ctx,
        "file_grants",
        "file_grant",
        id,
        expected,
        vec![("revoked_at", ctx.now.to_string().into())],
    )?;
    tx.commit()?;
    Ok(())
}

/// Re-validates a grant on every use: not revoked, still canonical-equal (so a moved or
/// swapped target is denied), and of the expected purpose.
pub fn grant_resolve(conn: &Connection, id: &str, purposes: &[&str]) -> Res<PathBuf> {
    let denied = || {
        IpcError::new(
            "ProviderDenied",
            "file grant is missing, revoked or changed",
        )
    };
    let row: Option<(String, String, Option<String>)> = conn
        .query_row(
            "SELECT path, purpose, revoked_at FROM file_grants WHERE id=?1 AND deleted_at IS NULL",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (path, purpose, revoked) = row.ok_or_else(denied)?;
    if revoked.is_some() || !purposes.contains(&purpose.as_str()) {
        return Err(denied());
    }
    let again = canonical(&path, purpose != "export").map_err(|_| denied())?;
    if again.to_string_lossy() != path {
        return Err(denied());
    }
    Ok(again)
}

/// Reads a granted import file with a hard size cap (no content indexing).
pub fn read_granted(conn: &Connection, grant_id: &str) -> Res<String> {
    let p = grant_resolve(conn, grant_id, &["import"])?;
    let meta = std::fs::metadata(&p).map_err(|_| val("cannot read granted file"))?;
    if meta.len() > MAX_IMPORT_BYTES {
        return Err(val("file exceeds the 5 MiB import limit"));
    }
    String::from_utf8(std::fs::read(&p).map_err(|_| val("cannot read granted file"))?)
        .map_err(|_| val("file is not valid UTF-8 text"))
}

pub fn write_granted(conn: &Connection, grant_id: &str, content: &str) -> Res<()> {
    let p = grant_resolve(conn, grant_id, &["export"])?;
    std::fs::write(p, content).map_err(|_| IpcError::new("Unavailable", "cannot write export"))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkReq {
    pub owner_type: String,
    pub owner_id: String,
    pub grant_id: String,
    #[serde(default)]
    pub label: String,
}

/// `file_link_create(file_grant_id)`: attaches an already-granted file to an entity.
pub fn file_link_create(conn: &mut Connection, ctx: &Ctx, req: LinkReq) -> Res<Json> {
    let table = match req.owner_type.as_str() {
        "project" => "projects",
        "milestone" => "milestones",
        "practice_session" => "practice_sessions",
        "course" => "courses",
        "study_item" => "study_items",
        _ => return Err(val("unsupported owner_type")),
    };
    let label = rec::free_text(&req.label, "label", 200)?;
    rec::exists(conn, table, &req.owner_id)?;
    grant_resolve(conn, &req.grant_id, &["link", "document"])?;
    let tx = conn.transaction()?;
    let id = rec::insert(
        &tx,
        ctx,
        "file_links",
        "file_link",
        vec![
            ("owner_type", req.owner_type.into()),
            ("owner_id", req.owner_id.into()),
            ("grant_id", req.grant_id.into()),
            ("label", label.into()),
        ],
    )?;
    tx.commit()?;
    rec::get(conn, "file_links", &id)
}

pub fn file_link_list(conn: &Connection, owner_type: &str, owner_id: &str) -> Res<Vec<Json>> {
    rec::list(
        conn,
        "file_links",
        "AND owner_type=?1 AND owner_id=?2",
        vec![owner_type.to_string().into(), owner_id.to_string().into()],
        "created_at",
    )
}
