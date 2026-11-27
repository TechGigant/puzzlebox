//! SQLite persistence. A single connection behind a mutex is plenty for this
//! workload; all calls hop onto the blocking pool.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MIGRATIONS: &[&str] = &[
    r#"
CREATE TABLE programs (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    enabled     INTEGER NOT NULL DEFAULT 1,
    mode        TEXT NOT NULL DEFAULT 'single',
    max_runs    INTEGER NOT NULL DEFAULT 10,
    workspace   TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE TABLE kv (
    scope TEXT NOT NULL,
    key   TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (scope, key)
);
CREATE TABLE runs (
    id          TEXT PRIMARY KEY,
    program_id  TEXT NOT NULL,
    started_at  INTEGER NOT NULL,
    finished_at INTEGER,
    status      TEXT NOT NULL,
    error       TEXT,
    trigger     TEXT,
    trace       TEXT
);
CREATE INDEX runs_by_program ON runs(program_id, started_at DESC);
"#,
    r#"
ALTER TABLE programs ADD COLUMN folder TEXT NOT NULL DEFAULT '';
"#,
    r#"
CREATE TABLE program_versions (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    program_id  TEXT NOT NULL,
    saved_at    INTEGER NOT NULL,
    name        TEXT NOT NULL,
    workspace   TEXT NOT NULL
);
CREATE INDEX versions_by_program ON program_versions(program_id, id DESC);
"#,
    r#"
ALTER TABLE programs ADD COLUMN deleted_at INTEGER;
ALTER TABLE programs ADD COLUMN labels TEXT NOT NULL DEFAULT '{}';
ALTER TABLE runs ADD COLUMN summary TEXT;
"#,
    r#"
CREATE TABLE program_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    program_id  TEXT NOT NULL,
    ts          INTEGER NOT NULL,
    level       TEXT NOT NULL,
    message     TEXT NOT NULL,
    run_id      TEXT
);
CREATE INDEX log_by_program ON program_log(program_id, id DESC);
"#,
    r#"
ALTER TABLE program_versions ADD COLUMN mode TEXT;
"#,
];

/// Days a deleted program stays in the recycle bin.
pub const BIN_DAYS: i64 = 30;

/// Saved versions kept per program (restore points).
const VERSIONS_KEPT: i64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProgramRecord {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub enabled: bool,
    pub mode: String,
    pub max_runs: u32,
    pub workspace: Value,
    /// Folder on the program list ("" = none); "/" separates nested folders.
    #[serde(default)]
    pub folder: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProgramVersion {
    pub id: i64,
    pub saved_at: i64,
    pub name: String,
    /// Run mode when it was saved; `None` for versions saved before it was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Blocks in this version, not counting shadows.
    #[serde(default)]
    pub blocks: usize,
    /// What changed since the version before, e.g. "2 blocks added, 1 changed".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub changes: String,
    /// Only filled when one version is fetched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunRecord {
    pub id: String,
    pub program_id: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub status: String,
    pub error: Option<String>,
    pub trigger: Option<Value>,
    pub trace: Option<Value>,
    /// What started it, in words ("Movement turned on").
    #[serde(default)]
    pub summary: Option<String>,
}

/// A line of a program's log.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogRecord {
    pub program_id: String,
    pub ts: i64,
    pub level: String,
    pub message: String,
    pub run_id: Option<String>,
}

/// A program in the recycle bin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeletedProgram {
    pub id: String,
    pub name: String,
    pub folder: String,
    pub deleted_at: i64,
}

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

fn json_col(v: Option<String>) -> Option<Value> {
    v.and_then(|s| from_str_deep(&s).ok())
}

/// Parses JSON of any nesting depth. Blockly nests each block of a stack inside the previous
/// one (`next.block`), so a long stack goes far past serde_json's default limit of 128 levels;
/// serde_stacker grows the stack instead of overflowing it.
pub fn from_str_deep<T: serde::de::DeserializeOwned>(s: &str) -> serde_json::Result<T> {
    let mut de = serde_json::Deserializer::from_str(s);
    de.disable_recursion_limit();
    let value = T::deserialize(serde_stacker::Deserializer::new(&mut de))?;
    de.end()?;
    Ok(value)
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        let version: usize = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        for (i, m) in MIGRATIONS.iter().enumerate().skip(version) {
            let tx = conn.transaction()?;
            tx.execute_batch(m)?;
            tx.pragma_update(None, "user_version", i + 1)?;
            tx.commit()?;
        }
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    async fn with<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock().unwrap_or_else(|p| p.into_inner());
            f(&mut guard)
        })
        .await?
    }

    pub async fn list_programs(&self) -> Result<Vec<ProgramRecord>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, name, description, enabled, mode, max_runs, workspace, created_at, updated_at, folder
                 FROM programs WHERE deleted_at IS NULL ORDER BY name COLLATE NOCASE",
            )?;
            let rows = st.query_map([], row_to_program)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
    }

    pub async fn get_program(&self, id: &str) -> Result<Option<ProgramRecord>> {
        let id = id.to_owned();
        self.with(move |c| {
            Ok(c.query_row(
                "SELECT id, name, description, enabled, mode, max_runs, workspace, created_at, updated_at, folder
                 FROM programs WHERE id = ?1 AND deleted_at IS NULL",
                [id],
                row_to_program,
            )
            .optional()?)
        })
        .await
    }

    /// Inserts or updates; returns the stored record with timestamps filled in.
    pub async fn save_program(&self, mut p: ProgramRecord) -> Result<ProgramRecord> {
        self.with(move |c| {
            let now = now_ms();
            let created: Option<i64> =
                c.query_row("SELECT created_at FROM programs WHERE id = ?1", [&p.id], |r| r.get(0)).optional()?;
            p.created_at = created.unwrap_or(now);
            p.updated_at = now;
            c.execute(
                "INSERT INTO programs (id, name, description, enabled, mode, max_runs, workspace, created_at, updated_at, folder)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(id) DO UPDATE SET name=excluded.name, description=excluded.description,
                   enabled=excluded.enabled, mode=excluded.mode, max_runs=excluded.max_runs,
                   workspace=excluded.workspace, updated_at=excluded.updated_at, folder=excluded.folder",
                params![
                    p.id,
                    p.name,
                    p.description,
                    p.enabled,
                    p.mode,
                    p.max_runs,
                    p.workspace.to_string(),
                    p.created_at,
                    p.updated_at,
                    p.folder
                ],
            )?;
            // A restore point for every save that changed the blocks, the name or the run mode.
            let ws = p.workspace.to_string();
            let last: Option<(String, String, Option<String>)> = c
                .query_row(
                    "SELECT workspace, name, mode FROM program_versions WHERE program_id = ?1 ORDER BY id DESC LIMIT 1",
                    [&p.id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let unchanged = last.as_ref().is_some_and(|(w, name, mode)| {
                *w == ws && *name == p.name && mode.as_deref().is_none_or(|m| m == p.mode)
            });
            if !unchanged {
                c.execute(
                    "INSERT INTO program_versions (program_id, saved_at, name, workspace, mode) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![p.id, p.updated_at, p.name, ws, p.mode],
                )?;
                c.execute(
                    "DELETE FROM program_versions WHERE program_id = ?1 AND id NOT IN
                       (SELECT id FROM program_versions WHERE program_id = ?1 ORDER BY id DESC LIMIT ?2)",
                    params![p.id, VERSIONS_KEPT],
                )?;
            }
            Ok(p)
        })
        .await
    }

    /// Restore points, newest first (without their blocks).
    pub async fn list_versions(&self, program_id: &str) -> Result<Vec<ProgramVersion>> {
        let id = program_id.to_owned();
        self.with(move |c| {
            let mut st = c.prepare(
                "SELECT id, saved_at, name, workspace, mode FROM program_versions WHERE program_id = ?1 ORDER BY id",
            )?;
            let rows = st.query_map([id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })?;
            let mut out = Vec::new();
            let mut prev: Option<(BTreeMap<String, Value>, String, Option<String>)> = None;
            for row in rows {
                let (id, saved_at, name, ws, mode) = row?;
                let blocks = block_map(&from_str_deep(&ws).unwrap_or(Value::Null));
                let changes = match &prev {
                    None => "first save".to_owned(),
                    Some((before, before_name, before_mode)) => {
                        let text = describe_changes(before, &blocks, before_name, &name);
                        let mode_changed = before_mode.is_some() && mode.is_some() && *before_mode != mode;
                        match (mode_changed, *before == blocks && before_name == &name) {
                            (false, _) => text,
                            (true, true) => "run mode changed".to_owned(),
                            (true, false) => format!("{text}, run mode changed"),
                        }
                    }
                };
                out.push(ProgramVersion {
                    id,
                    saved_at,
                    name: name.clone(),
                    mode: mode.clone(),
                    blocks: blocks.len(),
                    changes,
                    workspace: None,
                });
                prev = Some((blocks, name, mode.clone()));
            }
            out.reverse();
            Ok(out)
        })
        .await
    }

    pub async fn get_version(&self, program_id: &str, version: i64) -> Result<Option<ProgramVersion>> {
        let id = program_id.to_owned();
        self.with(move |c| {
            Ok(c.query_row(
                "SELECT id, saved_at, name, workspace, mode FROM program_versions WHERE program_id = ?1 AND id = ?2",
                params![id, version],
                |r| {
                    let ws: String = r.get(3)?;
                    let workspace: Option<Value> = from_str_deep(&ws).ok();
                    Ok(ProgramVersion {
                        id: r.get(0)?,
                        saved_at: r.get(1)?,
                        name: r.get(2)?,
                        mode: r.get(4)?,
                        blocks: workspace.as_ref().map(|w| block_map(w).len()).unwrap_or_default(),
                        changes: String::new(),
                        workspace,
                    })
                },
            )
            .optional()?)
        })
        .await
    }

    /// Moves a program to the recycle bin; its runs, versions and variables stay until it is
    /// purged.
    pub async fn delete_program(&self, id: &str) -> Result<bool> {
        let id = id.to_owned();
        self.with(move |c| {
            let n = c.execute(
                "UPDATE programs SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![id, now_ms()],
            )?;
            Ok(n > 0)
        })
        .await
    }

    /// Takes a program out of the recycle bin.
    pub async fn restore_program(&self, id: &str) -> Result<Option<ProgramRecord>> {
        let key = id.to_owned();
        let n = self
            .with(move |c| {
                Ok(c.execute(
                    "UPDATE programs SET deleted_at = NULL WHERE id = ?1 AND deleted_at IS NOT NULL",
                    [key],
                )?)
            })
            .await?;
        if n == 0 {
            return Ok(None);
        }
        self.get_program(id).await
    }

    /// Programs in the recycle bin, most recently deleted first.
    pub async fn list_deleted(&self) -> Result<Vec<DeletedProgram>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, name, folder, deleted_at FROM programs WHERE deleted_at IS NOT NULL
                 ORDER BY deleted_at DESC",
            )?;
            let rows = st.query_map([], |r| {
                Ok(DeletedProgram {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    folder: r.get(2)?,
                    deleted_at: r.get(3)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
    }

    /// Deletes a program in the recycle bin for good, with everything it kept.
    pub async fn purge_program(&self, id: &str) -> Result<bool> {
        let id = id.to_owned();
        self.with(move |c| {
            let tx = c.transaction()?;
            let n = tx.execute("DELETE FROM programs WHERE id = ?1 AND deleted_at IS NOT NULL", [&id])?;
            if n > 0 {
                purge_rest(&tx, &id)?;
            }
            tx.commit()?;
            Ok(n > 0)
        })
        .await
    }

    /// Empties the bin of programs deleted more than `BIN_DAYS` ago, and drops the stray
    /// "New program"s older versions created on every click (still named so, no blocks, never
    /// saved again, never run) created more than `strays_after_ms` ago. Returns the ids removed.
    pub async fn purge_old(&self, strays_after_ms: i64) -> Result<Vec<String>> {
        self.with(move |c| {
            let now = now_ms();
            let cutoff = now - BIN_DAYS * 24 * 3600 * 1000;
            let mut ids: Vec<String> = {
                let mut st =
                    c.prepare("SELECT id FROM programs WHERE deleted_at IS NOT NULL AND deleted_at < ?1")?;
                st.query_map([cutoff], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
            };
            {
                let mut st = c.prepare(
                    "SELECT id, workspace FROM programs p WHERE created_at = updated_at AND description = ''
                     AND name = 'New program' AND deleted_at IS NULL AND created_at <= ?1
                     AND NOT EXISTS (SELECT 1 FROM runs WHERE program_id = p.id)",
                )?;
                let rows = st.query_map([now - strays_after_ms], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?;
                for row in rows {
                    let (id, ws) = row?;
                    // A workspace that doesn't parse isn't known to be empty.
                    if from_str_deep::<Value>(&ws).is_ok_and(|w| block_map(&w).is_empty()) {
                        ids.push(id);
                    }
                }
            }
            let tx = c.transaction()?;
            for id in &ids {
                tx.execute("DELETE FROM programs WHERE id = ?1", [id])?;
                purge_rest(&tx, id)?;
            }
            tx.commit()?;
            Ok(ids)
        })
        .await
    }

    /// Appends log lines and keeps only the newest `keep` per program. Lines of programs that
    /// no longer exist (purged meanwhile) are dropped.
    pub async fn append_log(&self, lines: Vec<LogRecord>, keep: usize) -> Result<()> {
        if lines.is_empty() {
            return Ok(());
        }
        self.with(move |c| {
            let tx = c.transaction()?;
            let mut programs = std::collections::BTreeSet::new();
            {
                let mut st = tx.prepare(
                    "INSERT INTO program_log (program_id, ts, level, message, run_id)
                     SELECT ?1, ?2, ?3, ?4, ?5 WHERE EXISTS (SELECT 1 FROM programs WHERE id = ?1)",
                )?;
                for l in &lines {
                    st.execute(params![l.program_id, l.ts, l.level, l.message, l.run_id])?;
                    programs.insert(l.program_id.as_str());
                }
            }
            for p in programs {
                tx.execute(
                    "DELETE FROM program_log WHERE program_id = ?1 AND id NOT IN
                       (SELECT id FROM program_log WHERE program_id = ?1 ORDER BY id DESC LIMIT ?2)",
                    params![p, keep as i64],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Every program's log (in the bin too), oldest line first.
    pub async fn all_logs(&self) -> Result<Vec<LogRecord>> {
        self.with(|c| {
            let mut st =
                c.prepare("SELECT program_id, ts, level, message, run_id FROM program_log ORDER BY id")?;
            let rows = st.query_map([], |r| {
                Ok(LogRecord {
                    program_id: r.get(0)?,
                    ts: r.get(1)?,
                    level: r.get(2)?,
                    message: r.get(3)?,
                    run_id: r.get(4)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
    }

    /// Stores the editor's text for each block (see `labels`).
    pub async fn save_labels(&self, id: &str, labels: &Value) -> Result<()> {
        let (id, labels) = (id.to_owned(), labels.to_string());
        self.with(move |c| {
            c.execute("UPDATE programs SET labels = ?2 WHERE id = ?1", params![id, labels])?;
            Ok(())
        })
        .await
    }

    /// Block id → the block's text in the editor when the program was last saved, so runs
    /// describe their steps the way the blocks read when they ran.
    pub async fn labels(&self, id: &str) -> Result<Value> {
        let id = id.to_owned();
        self.with(move |c| {
            let s: Option<String> =
                c.query_row("SELECT labels FROM programs WHERE id = ?1", [id], |r| r.get(0)).optional()?;
            Ok(s.and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| Value::Object(Default::default())))
        })
        .await
    }

    pub async fn kv_all(&self, scope: &str) -> Result<serde_json::Map<String, Value>> {
        let scope = scope.to_owned();
        self.with(move |c| {
            let mut st = c.prepare("SELECT key, value FROM kv WHERE scope = ?1")?;
            let rows = st.query_map([scope], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            let mut out = serde_json::Map::new();
            for row in rows {
                let (k, v) = row?;
                out.insert(k, from_str_deep(&v).unwrap_or(Value::Null));
            }
            Ok(out)
        })
        .await
    }

    pub async fn kv_set(&self, scope: &str, key: &str, value: &Value) -> Result<()> {
        let (scope, key, value) = (scope.to_owned(), key.to_owned(), value.to_string());
        self.with(move |c| {
            c.execute(
                "INSERT INTO kv (scope, key, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT(scope, key) DO UPDATE SET value = excluded.value",
                params![scope, key, value],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn kv_delete(&self, scope: &str, key: &str) -> Result<()> {
        let (scope, key) = (scope.to_owned(), key.to_owned());
        self.with(move |c| {
            c.execute("DELETE FROM kv WHERE scope = ?1 AND key = ?2", params![scope, key])?;
            Ok(())
        })
        .await
    }

    /// Upserts a run and prunes history beyond `keep` runs for that program.
    pub async fn save_run(&self, r: RunRecord, keep: usize) -> Result<()> {
        self.with(move |c| {
            c.execute(
                "INSERT INTO runs (id, program_id, started_at, finished_at, status, error, trigger, trace, summary)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(id) DO UPDATE SET finished_at=excluded.finished_at, status=excluded.status,
                   error=excluded.error, trace=excluded.trace",
                params![
                    r.id,
                    r.program_id,
                    r.started_at,
                    r.finished_at,
                    r.status,
                    r.error,
                    r.trigger.map(|v| v.to_string()),
                    r.trace.map(|v| v.to_string()),
                    r.summary
                ],
            )?;
            c.execute(
                "DELETE FROM runs WHERE program_id = ?1 AND id NOT IN
                   (SELECT id FROM runs WHERE program_id = ?1 ORDER BY started_at DESC LIMIT ?2)",
                params![r.program_id, keep as i64],
            )?;
            Ok(())
        })
        .await
    }

    /// Lists runs without their (potentially large) traces.
    pub async fn list_runs(&self, program_id: &str, limit: usize) -> Result<Vec<RunRecord>> {
        let program_id = program_id.to_owned();
        self.with(move |c| {
            let mut st = c.prepare(
                "SELECT id, program_id, started_at, finished_at, status, error, trigger, summary
                 FROM runs WHERE program_id = ?1 ORDER BY started_at DESC LIMIT ?2",
            )?;
            let rows = st.query_map(params![program_id, limit as i64], |r| {
                Ok(RunRecord {
                    id: r.get(0)?,
                    program_id: r.get(1)?,
                    started_at: r.get(2)?,
                    finished_at: r.get(3)?,
                    status: r.get(4)?,
                    error: r.get(5)?,
                    trigger: json_col(r.get(6)?),
                    trace: None,
                    summary: r.get(7)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
    }

    pub async fn get_run(&self, id: &str) -> Result<Option<RunRecord>> {
        let id = id.to_owned();
        self.with(move |c| {
            Ok(c.query_row(
                "SELECT id, program_id, started_at, finished_at, status, error, trigger, trace, summary
                 FROM runs WHERE id = ?1",
                [id],
                |r| {
                    Ok(RunRecord {
                        id: r.get(0)?,
                        program_id: r.get(1)?,
                        started_at: r.get(2)?,
                        finished_at: r.get(3)?,
                        status: r.get(4)?,
                        error: r.get(5)?,
                        trigger: json_col(r.get(6)?),
                        trace: json_col(r.get(7)?),
                        summary: r.get(8)?,
                    })
                },
            )
            .optional()?)
        })
        .await
    }
}

fn row_to_program(r: &rusqlite::Row<'_>) -> rusqlite::Result<ProgramRecord> {
    let ws: String = r.get(6)?;
    Ok(ProgramRecord {
        id: r.get(0)?,
        name: r.get(1)?,
        description: r.get(2)?,
        enabled: r.get(3)?,
        mode: r.get(4)?,
        max_runs: r.get(5)?,
        workspace: from_str_deep(&ws).unwrap_or(Value::Null),
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
        folder: r.get(9)?,
    })
}

fn purge_rest(tx: &rusqlite::Transaction<'_>, id: &str) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM runs WHERE program_id = ?1", [id])?;
    tx.execute("DELETE FROM program_versions WHERE program_id = ?1", [id])?;
    tx.execute("DELETE FROM program_log WHERE program_id = ?1", [id])?;
    tx.execute("DELETE FROM kv WHERE scope = ?1", [format!("program:{id}")])?;
    Ok(())
}

/// Every (non-shadow) block of a saved Blockly workspace by id, each without its position and
/// with the blocks inside it replaced by their ids, so a change shows up only on the block
/// that changed.
pub fn block_map(workspace: &Value) -> BTreeMap<String, Value> {
    fn walk(block: &Value, out: &mut BTreeMap<String, Value>) {
        let Some(b) = block.as_object() else { return };
        let mut own = b.clone();
        own.remove("x");
        own.remove("y");
        if let Some(next) = b.get("next").and_then(|n| n.get("block")) {
            own.insert("next".into(), next.get("id").cloned().unwrap_or_default());
            walk(next, out);
        }
        if let Some(inputs) = b.get("inputs").and_then(Value::as_object) {
            let mut own_inputs = serde_json::Map::new();
            for (name, input) in inputs {
                let mut i = input.clone();
                if let Some(child) = input.get("block") {
                    i["block"] = child.get("id").cloned().unwrap_or_default();
                    walk(child, out);
                }
                own_inputs.insert(name.clone(), i);
            }
            own.insert("inputs".into(), Value::Object(own_inputs));
        }
        let id = b.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
        out.insert(id, Value::Object(own));
    }
    let mut out = BTreeMap::new();
    for top in workspace.pointer("/blocks/blocks").and_then(Value::as_array).into_iter().flatten() {
        walk(top, &mut out);
    }
    out
}

fn describe_changes(
    before: &BTreeMap<String, Value>,
    after: &BTreeMap<String, Value>,
    before_name: &str,
    name: &str,
) -> String {
    let added = after.keys().filter(|k| !before.contains_key(*k)).count();
    let removed = before.keys().filter(|k| !after.contains_key(*k)).count();
    let changed = after.iter().filter(|(k, v)| before.get(*k).is_some_and(|b| b != *v)).count();
    let blocks = |n: usize| if n == 1 { "1 block".to_owned() } else { format!("{n} blocks") };
    let mut parts = Vec::new();
    if added > 0 {
        parts.push(format!("{} added", blocks(added)));
    }
    if removed > 0 {
        parts.push(if parts.is_empty() {
            format!("{} removed", blocks(removed))
        } else {
            format!("{removed} removed")
        });
    }
    if changed > 0 {
        parts.push(if parts.is_empty() {
            format!("{} changed", blocks(changed))
        } else {
            format!("{changed} changed")
        });
    }
    if before_name != name {
        parts.push(format!("renamed from “{before_name}”"));
    }
    if parts.is_empty() {
        parts.push("blocks moved".into());
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn program(id: &str) -> ProgramRecord {
        ProgramRecord {
            folder: String::new(),
            id: id.into(),
            name: format!("P {id}"),
            description: String::new(),
            enabled: true,
            mode: "single".into(),
            max_runs: 10,
            workspace: json!({"blocks": {}}),
            created_at: 0,
            updated_at: 0,
        }
    }

    #[tokio::test]
    async fn program_crud_and_cascade() {
        let s = Store::open_in_memory().unwrap();
        let saved = s.save_program(program("a")).await.unwrap();
        assert!(saved.created_at > 0);
        let mut changed = saved.clone();
        changed.name = "Renamed".into();
        let again = s.save_program(changed).await.unwrap();
        assert_eq!(again.created_at, saved.created_at);
        assert_eq!(s.get_program("a").await.unwrap().unwrap().name, "Renamed");

        s.kv_set("program:a", "count", &json!(3)).await.unwrap();
        assert_eq!(s.kv_all("program:a").await.unwrap()["count"], 3);
        assert!(s.delete_program("a").await.unwrap());
        assert!(s.list_programs().await.unwrap().is_empty());
        assert!(s.get_program("a").await.unwrap().is_none());
        // In the bin it keeps its variables and can come back.
        assert_eq!(s.list_deleted().await.unwrap()[0].name, "Renamed");
        assert_eq!(s.kv_all("program:a").await.unwrap()["count"], 3);
        assert_eq!(s.restore_program("a").await.unwrap().unwrap().name, "Renamed");
        assert!(s.list_deleted().await.unwrap().is_empty());

        assert!(!s.purge_program("a").await.unwrap(), "only programs in the bin are purged");
        s.delete_program("a").await.unwrap();
        assert!(s.purge_program("a").await.unwrap());
        assert!(s.kv_all("program:a").await.unwrap().is_empty());
        assert!(s.list_deleted().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn untouched_empty_programs_are_dropped() {
        let s = Store::open_in_memory().unwrap();
        let mut empty = program("empty");
        empty.name = "New program".into();
        s.save_program(empty).await.unwrap();
        // Named, so the user meant to keep it even without blocks.
        let mut named = program("named");
        named.name = "Kitchen".into();
        s.save_program(named).await.unwrap();
        let mut used = program("used");
        used.workspace = json!({"blocks": {"blocks": [{"type": "ha_when", "id": "w"}]}});
        s.save_program(used).await.unwrap();
        // Strays are kept for a while: the editor may still have one open.
        assert!(s.purge_old(3_600_000).await.unwrap().is_empty());
        assert_eq!(s.purge_old(0).await.unwrap(), ["empty"]);
        let left: Vec<_> = s.list_programs().await.unwrap().into_iter().map(|p| p.id).collect();
        assert_eq!(left.len(), 2);
        assert!(left.contains(&"named".to_owned()) && left.contains(&"used".to_owned()));
    }

    #[tokio::test]
    async fn logs_are_bounded_kept_in_the_bin_and_purged() {
        let s = Store::open_in_memory().unwrap();
        s.save_program(program("a")).await.unwrap();
        s.save_program(program("b")).await.unwrap();
        let line = |p: &str, i: i64| LogRecord {
            program_id: p.into(),
            ts: i,
            level: "info".into(),
            message: format!("line {i}"),
            run_id: None,
        };
        s.append_log((0..5).map(|i| line("a", i)).collect(), 3).await.unwrap();
        s.append_log(vec![line("b", 9), line("gone", 1)], 3).await.unwrap();
        let ts = |logs: &[LogRecord], p: &str| {
            logs.iter().filter(|l| l.program_id == p).map(|l| l.ts).collect::<Vec<_>>()
        };
        let logs = s.all_logs().await.unwrap();
        assert_eq!(ts(&logs, "a"), [2, 3, 4]);
        assert_eq!(ts(&logs, "b"), [9]);
        assert!(ts(&logs, "gone").is_empty(), "lines of unknown programs are dropped");

        s.delete_program("a").await.unwrap();
        assert_eq!(ts(&s.all_logs().await.unwrap(), "a").len(), 3);
        s.purge_program("a").await.unwrap();
        assert!(ts(&s.all_logs().await.unwrap(), "a").is_empty());
    }

    #[test]
    fn version_changes_are_described() {
        let a = block_map(&json!({"blocks": {"blocks": [
            {"type": "ha_when", "id": "w", "x": 1, "inputs": {"DO": {"block": {"type": "log", "id": "l",
                "inputs": {"MSG": {"shadow": {"type": "text", "id": "s", "fields": {"TEXT": "hi"}}}}}}}}
        ]}}));
        assert_eq!(a.len(), 2);
        let b = block_map(&json!({"blocks": {"blocks": [
            {"type": "ha_when", "id": "w", "x": 50, "inputs": {"DO": {"block": {"type": "log", "id": "l",
                "inputs": {"MSG": {"shadow": {"type": "text", "id": "s", "fields": {"TEXT": "bye"}}}},
                "next": {"block": {"type": "stop", "id": "x"}}}}}}
        ]}}));
        assert_eq!(describe_changes(&a, &b, "P", "P"), "1 block added, 1 changed");
        assert_eq!(describe_changes(&b, &a, "P", "Q"), "1 block removed, 1 changed, renamed from “P”");
        assert_eq!(describe_changes(&a, &a, "P", "P"), "blocks moved");
    }

    #[tokio::test]
    async fn run_history_is_pruned() {
        let s = Store::open_in_memory().unwrap();
        for i in 0..5 {
            let run = RunRecord {
                id: format!("r{i}"),
                program_id: "p".into(),
                started_at: i,
                finished_at: Some(i + 1),
                status: "ok".into(),
                error: None,
                trigger: Some(json!({"i": i})),
                trace: Some(json!([i])),
                summary: None,
            };
            s.save_run(run, 3).await.unwrap();
        }
        let runs = s.list_runs("p", 10).await.unwrap();
        assert_eq!(runs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["r4", "r3", "r2"]);
        assert_eq!(s.get_run("r4").await.unwrap().unwrap().trace, Some(json!([4])));
    }

    #[tokio::test]
    async fn long_stacks_round_trip() {
        let s = Store::open_in_memory().unwrap();
        let mut stack = json!({"type": "text_print"});
        for _ in 0..500 {
            stack = json!({"type": "text_print", "next": {"block": stack}});
        }
        let mut p = program("deep");
        p.workspace = json!({"blocks": {"blocks": [stack]}});
        s.save_program(p.clone()).await.unwrap();
        assert_eq!(s.get_program("deep").await.unwrap().unwrap().workspace, p.workspace);
    }

    #[tokio::test]
    async fn folder_round_trips() {
        let s = Store::open_in_memory().unwrap();
        let mut p = program("f1");
        p.folder = "Lights/Outside".into();
        s.save_program(p).await.unwrap();
        assert_eq!(s.get_program("f1").await.unwrap().unwrap().folder, "Lights/Outside");
    }

    #[tokio::test]
    async fn versions_are_kept_per_change() {
        let s = Store::open_in_memory().unwrap();
        let mut p = program("v1");
        for n in 0..3 {
            p.workspace = serde_json::json!({"n": n});
            s.save_program(p.clone()).await.unwrap();
        }
        // Saving without changes adds no restore point.
        s.save_program(p.clone()).await.unwrap();
        let versions = s.list_versions("v1").await.unwrap();
        assert_eq!(versions.len(), 3);
        let first = s.get_version("v1", versions[2].id).await.unwrap().unwrap();
        assert_eq!(first.workspace, Some(serde_json::json!({"n": 0})));
        assert_eq!(versions[2].changes, "first save");
        assert_eq!(first.mode.as_deref(), Some("single"));
        // A new run mode or name is a restore point too, so restoring brings them back.
        p.mode = "queued".into();
        s.save_program(p.clone()).await.unwrap();
        p.name = "Renamed".into();
        s.save_program(p.clone()).await.unwrap();
        let versions = s.list_versions("v1").await.unwrap();
        assert_eq!(versions.len(), 5);
        assert_eq!(versions[1].changes, "run mode changed");
        assert_eq!(versions[1].mode.as_deref(), Some("queued"));
        assert!(versions[0].changes.contains("renamed from"), "{}", versions[0].changes);
        s.delete_program("v1").await.unwrap();
        s.purge_program("v1").await.unwrap();
        assert!(s.list_versions("v1").await.unwrap().is_empty());
    }
}
