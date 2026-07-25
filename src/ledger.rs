use std::path::Path;

use anyhow::{Result, anyhow};
use chrono::Utc;
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use uuid::Uuid;

use crate::storage::{ensure_db, open_db};
use crate::util::timestamp;

#[derive(Debug, Serialize)]
pub(crate) struct Ledger {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) contents: String,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct LedgerSummary {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) content_bytes: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct LedgerMetadata {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

pub(crate) fn create(db_path: &Path, title: &str) -> Result<LedgerMetadata> {
    ensure_db(db_path)?;
    let mut conn = open_db(db_path)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let base = format!("{}-{}", slug(title), Utc::now().format("%Y%m%dT%H%M%SZ"));
    let mut id = base.clone();
    if tx
        .query_row("SELECT 1 FROM ledgers WHERE id = ?1", [&id], |_| Ok(()))
        .optional()?
        .is_some()
    {
        id = format!("{base}-{}", &Uuid::new_v4().simple().to_string()[..8]);
    }
    let now = timestamp();
    tx.execute(
        "INSERT INTO ledgers(id, title, contents, created_at, updated_at)
         VALUES(?1, ?2, '', ?3, ?3)",
        params![id, title, now],
    )?;
    tx.commit()?;
    Ok(LedgerMetadata {
        id,
        title: title.to_string(),
        created_at: now.clone(),
        updated_at: now,
    })
}

pub(crate) fn list(db_path: &Path) -> Result<Vec<LedgerSummary>> {
    ensure_db(db_path)?;
    let conn = open_db(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT id, title, created_at, updated_at, length(CAST(contents AS BLOB))
         FROM ledgers
         ORDER BY updated_at DESC, id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(LedgerSummary {
            id: row.get(0)?,
            title: row.get(1)?,
            created_at: row.get(2)?,
            updated_at: row.get(3)?,
            content_bytes: row.get::<_, i64>(4)? as usize,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub(crate) fn show(db_path: &Path, id: &str) -> Result<Ledger> {
    ensure_db(db_path)?;
    let conn = open_db(db_path)?;
    find(&conn, id)
}

pub(crate) fn write(db_path: &Path, id: &str, contents: String) -> Result<LedgerSummary> {
    ensure_db(db_path)?;
    let mut conn = open_db(db_path)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let ledger = find(&tx, id)?;
    let updated_at = timestamp();
    tx.execute(
        "UPDATE ledgers SET contents = ?1, updated_at = ?2 WHERE id = ?3",
        params![contents, updated_at, id],
    )?;
    tx.commit()?;
    Ok(LedgerSummary {
        id: ledger.id,
        title: ledger.title,
        created_at: ledger.created_at,
        updated_at,
        content_bytes: contents.len(),
    })
}

fn find(conn: &rusqlite::Connection, id: &str) -> Result<Ledger> {
    conn.query_row(
        "SELECT id, title, contents, created_at, updated_at FROM ledgers WHERE id = ?1",
        [id],
        |row| {
            Ok(Ledger {
                id: row.get(0)?,
                title: row.get(1)?,
                contents: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        },
    )
    .optional()?
    .ok_or_else(|| anyhow!("ledger not found: {id}"))
}

fn slug(title: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in title.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(character.to_ascii_lowercase());
            separator = false;
        } else {
            separator = true;
        }
    }
    if slug.is_empty() {
        "ledger".to_string()
    } else {
        slug
    }
}
