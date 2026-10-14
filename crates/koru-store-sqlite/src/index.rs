// SPDX-License-Identifier: MPL-2.0

//! The derived listing of a workspace's documents.
//!
//! A projection of the manifest, kept in SQL so that listing a workspace costs
//! a query rather than opening every document in it. **The manifest is the
//! truth**; this can be thrown away and rebuilt at any moment, and
//! [`reindex`](DocumentIndex::reindex) does exactly that rather than diffing —
//! a diff that drifts is a listing that quietly stops matching the document,
//! and nothing would report it.
//!
//! It is its own port for a reason that has not arrived yet: this is the seam a
//! real full-text index appears at. Language-specific tokenisation is then a
//! reindex behind this trait rather than a migration of stored state, which is
//! what keeps the choice of tokenizer out of the canonical CRDT state.

use koru_store_core::errors::Result;
use koru_store_core::ids::DocumentId;
use koru_store_core::model::DocumentEntry;
use koru_store_core::ports::DocumentIndex;
use rusqlite::{OptionalExtension, params};

use crate::catalog::collect;
use crate::db::sql_failed;
use crate::workspace::SqliteWorkspace;

impl DocumentIndex for SqliteWorkspace {
    fn reindex(&mut self, entries: &[DocumentEntry]) -> Result<()> {
        // Replace wholesale, in one transaction: a half-replaced listing is
        // worse than a stale one, because it looks current.
        let tx = self.connection_mut().transaction().map_err(sql_failed)?;
        tx.execute("DELETE FROM documents", [])
            .map_err(sql_failed)?;
        {
            let mut insert = tx
                .prepare(
                    "INSERT INTO documents                      (doc_id, parent_id, ordinal, title, kind, mime, position, trashed_at, trashed_by, note_doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                )
                .map_err(sql_failed)?;
            for (position, e) in entries.iter().enumerate() {
                insert
                    .execute(params![
                        e.id.to_storage(),
                        e.parent.map(|p| p.to_storage()),
                        e.ordinal,
                        e.title,
                        e.kind,
                        e.mime.as_deref(),
                        position as i64,
                        e.trashed_at.map(|t| t.millis()),
                        e.trashed_by.as_deref(),
                        e.note.map(|n| n.to_storage()),
                    ])
                    .map_err(sql_failed)?;
            }
        }
        tx.commit().map_err(sql_failed)?;
        Ok(())
    }

    fn entries(&self) -> Result<Vec<DocumentEntry>> {
        // `position` is the manifest's walk order, so the listing comes back in
        // the order a person put the documents in rather than in id order.
        let mut stmt = self
            .connection()
            .prepare(
                "SELECT doc_id, parent_id, ordinal, title, kind, mime, trashed_at, trashed_by, note_doc
                 FROM documents ORDER BY position",
            )
            .map_err(sql_failed)?;
        let rows = stmt.query_map([], read_entry).map_err(sql_failed)?;
        collect(rows)
    }

    fn entry(&self, id: DocumentId) -> Result<Option<DocumentEntry>> {
        self.connection()
            .query_row(
                "SELECT doc_id, parent_id, ordinal, title, kind, mime, trashed_at, trashed_by, note_doc
                 FROM documents WHERE doc_id = ?1",
                params![id.to_storage()],
                read_entry,
            )
            .optional()
            .map_err(sql_failed)?
            .transpose()
    }
}

fn read_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<DocumentEntry>> {
    let id: String = row.get(0)?;
    let parent: Option<String> = row.get(1)?;
    let ordinal: i64 = row.get(2)?;
    let title: String = row.get(3)?;
    let kind: String = row.get(4)?;
    let mime: Option<String> = row.get(5)?;
    let trashed_at: Option<i64> = row.get(6)?;
    let trashed_by: Option<String> = row.get(7)?;
    let note: Option<String> = row.get(8)?;
    Ok((|| {
        use koru_store_core::errors::{Code, Error};
        Ok(DocumentEntry {
            id: DocumentId::parse(&id)
                .ok_or_else(|| Error::new(Code::CorruptState, "document id is not a uuid"))?,
            parent: parent.as_deref().and_then(DocumentId::parse),
            ordinal: ordinal.max(0) as u32,
            title,
            kind,
            mime,
            note: note.as_deref().and_then(DocumentId::parse),
            trashed_at: trashed_at.map(koru_store_core::model::Timestamp::from_millis),
            trashed_by,
        })
    })())
}

#[cfg(test)]
mod tests;
