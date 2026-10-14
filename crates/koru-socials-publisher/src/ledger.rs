// SPDX-License-Identifier: MPL-2.0

use crate::{Attempt, Fault, Journal, Key, Phase};
use koru_socials::{Publication, PublicationTarget};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use uuid::Uuid;

pub struct SqliteLedger {
    db: Connection,
}
impl SqliteLedger {
    pub fn open(path: &Path) -> Result<Self, Fault> {
        if cfg!(test) || cfg!(feature = "no-network") {
            return Err(Fault::Ledger);
        }
        Self::from_connection(Connection::open(path).map_err(|_| Fault::Ledger)?)
    }
    pub fn from_connection(db: Connection) -> Result<Self, Fault> {
        db.busy_timeout(std::time::Duration::from_secs(1))
            .map_err(|_| Fault::Ledger)?;
        db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")
            .map_err(|_| Fault::Ledger)?;
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|_| Fault::Ledger)?;
        if version == 0 {
            // An existing unknown database is not a fresh ledger.
            let tables:i64=db.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",[],|r|r.get(0)).map_err(|_|Fault::Ledger)?;
            if tables != 0 {
                return Err(Fault::Ledger);
            }
            db.execute_batch(concat!(
                "BEGIN IMMEDIATE;",
                include_str!("../migrations/socials_delivery_v1.sql"),
                "COMMIT;"
            ))
            .map_err(|_| Fault::Ledger)?;
        } else if version != 1 && version != 2 {
            return Err(Fault::Ledger);
        }
        if version < 2 {
            db.execute_batch(concat!(
                "BEGIN IMMEDIATE;",
                include_str!("../migrations/socials_delivery_v2.sql"),
                "COMMIT;"
            ))
            .map_err(|_| Fault::Ledger)?;
        }
        Ok(Self { db })
    }
}
fn phase(p: Phase) -> &'static str {
    match p {
        Phase::Ready => "ready",
        Phase::Sending => "sending",
        Phase::Published => "published",
        Phase::Failed => "failed",
        Phase::NeedsConnection => "needs_connection",
        Phase::Unconfirmed => "unconfirmed",
        Phase::Cancelled => "cancelled",
    }
}
fn decode_phase(p: &str) -> Result<Phase, Fault> {
    Ok(match p {
        "ready" => Phase::Ready,
        "sending" => Phase::Sending,
        "published" => Phase::Published,
        "failed" => Phase::Failed,
        "needs_connection" => Phase::NeedsConnection,
        "unconfirmed" => Phase::Unconfirmed,
        "cancelled" => Phase::Cancelled,
        _ => return Err(Fault::Ledger),
    })
}
impl Journal for SqliteLedger {
    fn load(&mut self, key: Key) -> Result<Option<Attempt>, Fault> {
        let row=self.db.query_row("SELECT attempt,device,actor,phase,next_at_ms,failures,request,error FROM social_attempts WHERE workspace=?1 AND post=?2 AND channel=?3",params![key.workspace.as_bytes().as_slice(),key.post.as_bytes().as_slice(),key.channel.as_bytes().as_slice()],|r| Ok((r.get::<_,Vec<u8>>(0)?,r.get::<_,Vec<u8>>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,Option<i64>>(4)?,r.get::<_,u32>(5)?,r.get::<_,Option<Vec<u8>>>(6)?,r.get::<_,Option<String>>(7)?))).optional().map_err(|_|Fault::Ledger)?;
        let Some((id, device, actor, p, next_at_ms, failures, request, error)) = row else {
            return Ok(None);
        };
        let mut a = Attempt {
            target: self.db.query_row("SELECT network,subreddit,reddit_title,flair_id FROM social_attempts WHERE workspace=?1 AND post=?2 AND channel=?3", params![key.workspace.as_bytes().as_slice(),key.post.as_bytes().as_slice(),key.channel.as_bytes().as_slice()], |r| Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,Option<String>>(3)?)))
                .map_err(|_| Fault::Ledger).and_then(|(network,sub,title,flair)| match network.as_str() {
                    "x" => Ok(PublicationTarget::X),
                    "linkedin_person" => Ok(PublicationTarget::LinkedInPerson),
                    "linkedin_page" => Ok(PublicationTarget::LinkedInPage),
                    "reddit" => Ok(PublicationTarget::Reddit { subreddit: sub.ok_or(Fault::Ledger)?, title: title.ok_or(Fault::Ledger)?, flair_id: flair }),
                    _ => Err(Fault::Ledger),
                })?,
            id: Uuid::from_slice(&id).map_err(|_| Fault::Ledger)?,
            device: Uuid::from_slice(&device).map_err(|_| Fault::Ledger)?,
            actor,
            parts: vec![],
            published: vec![],
            phase: decode_phase(&p)?,
            next_at_ms,
            failures,
            request: request
                .map(|v| Uuid::from_slice(&v).map_err(|_| Fault::Ledger))
                .transpose()?,
            error,
        };
        let mut stmt=self.db.prepare("SELECT part,text,remote_id,remote_url,published_at_ms FROM social_parts WHERE workspace=?1 AND post=?2 AND channel=?3 ORDER BY part").map_err(|_|Fault::Ledger)?;
        let rows = stmt
            .query_map(
                params![
                    key.workspace.as_bytes().as_slice(),
                    key.post.as_bytes().as_slice(),
                    key.channel.as_bytes().as_slice()
                ],
                |r| {
                    Ok((
                        r.get::<_, usize>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<i64>>(4)?,
                    ))
                },
            )
            .map_err(|_| Fault::Ledger)?;
        for row in rows {
            let (i, text, id, url, at) = row.map_err(|_| Fault::Ledger)?;
            if i != a.parts.len() {
                return Err(Fault::Ledger);
            }
            a.parts.push(text);
            if let Some(id) = id {
                if i != a.published.len() {
                    return Err(Fault::Ledger);
                }
                a.published.push(Publication {
                    id,
                    url,
                    published_at_ms: at.ok_or(Fault::Ledger)?,
                });
            }
        }
        Ok(Some(a))
    }
    fn store(&mut self, key: Key, a: &Attempt) -> Result<(), Fault> {
        if a.published.len() > a.parts.len() {
            return Err(Fault::Ledger);
        }
        if let Some(old) = self.load(key)? {
            // Frozen payload and confirmed prefix are append-only facts.
            if old.id != a.id
                || old.device != a.device
                || old.actor != a.actor
                || old.parts != a.parts
                || old.target != a.target
                || !a.published.starts_with(&old.published)
            {
                return Err(Fault::Ledger);
            }
        }
        let tx = self.db.transaction().map_err(|_| Fault::Ledger)?;
        tx.execute("INSERT INTO social_attempts(workspace,post,channel,attempt,device,actor,phase,next_at_ms,failures,request,error) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(workspace,post,channel) DO UPDATE SET phase=excluded.phase,next_at_ms=excluded.next_at_ms,failures=excluded.failures,request=excluded.request,error=excluded.error",params![key.workspace.as_bytes().as_slice(),key.post.as_bytes().as_slice(),key.channel.as_bytes().as_slice(),a.id.as_bytes().as_slice(),a.device.as_bytes().as_slice(),a.actor,phase(a.phase),a.next_at_ms,a.failures,a.request.map(|v|v.as_bytes().to_vec()),a.error]).map_err(|_|Fault::Ledger)?;
        let (network, sub, title, flair) = match &a.target {
            PublicationTarget::X => ("x", None, None, None),
            PublicationTarget::LinkedInPerson => ("linkedin_person", None, None, None),
            PublicationTarget::LinkedInPage => ("linkedin_page", None, None, None),
            PublicationTarget::Reddit {
                subreddit,
                title,
                flair_id,
            } => (
                "reddit",
                Some(subreddit.as_str()),
                Some(title.as_str()),
                flair_id.as_deref(),
            ),
        };
        tx.execute("UPDATE social_attempts SET network=?4,subreddit=?5,reddit_title=?6,flair_id=?7 WHERE workspace=?1 AND post=?2 AND channel=?3",params![key.workspace.as_bytes().as_slice(),key.post.as_bytes().as_slice(),key.channel.as_bytes().as_slice(),network,sub,title,flair]).map_err(|_| Fault::Ledger)?;
        for (i, text) in a.parts.iter().enumerate() {
            let p = a.published.get(i);
            tx.execute("INSERT INTO social_parts(workspace,post,channel,part,text,remote_id,remote_url,published_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(workspace,post,channel,part) DO UPDATE SET remote_id=excluded.remote_id,remote_url=excluded.remote_url,published_at_ms=excluded.published_at_ms",params![key.workspace.as_bytes().as_slice(),key.post.as_bytes().as_slice(),key.channel.as_bytes().as_slice(),i,text,p.map(|p|p.id.as_str()),p.and_then(|p|p.url.as_deref()),p.map(|p|p.published_at_ms)]).map_err(|_|Fault::Ledger)?;
        }
        tx.commit().map_err(|_| Fault::Ledger)
    }
}

#[cfg(test)]
mod tests;
