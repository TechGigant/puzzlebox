//! SQLite persistence. A single connection behind a mutex is plenty for this
//! workload; all calls hop onto the blocking pool.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MIGRATIONS: &[&str] = &[r#"
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
"#];

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
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
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
    v.and_then(|s| serde_json::from_str(&s).ok())
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
                "SELECT id, name, description, enabled, mode, max_runs, workspace, created_at, updated_at
                 FROM programs ORDER BY name COLLATE NOCASE",
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
                "SELECT id, name, description, enabled, mode, max_runs, workspace, created_at, updated_at
                 FROM programs WHERE id = ?1",
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
                "INSERT INTO programs (id, name, description, enabled, mode, max_runs, workspace, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(id) DO UPDATE SET name=excluded.name, description=excluded.description,
                   enabled=excluded.enabled, mode=excluded.mode, max_runs=excluded.max_runs,
                   workspace=excluded.workspace, updated_at=excluded.updated_at",
                params![
                    p.id,
                    p.name,
                    p.description,
                    p.enabled,
                    p.mode,
                    p.max_runs,
                    p.workspace.to_string(),
                    p.created_at,
                    p.updated_at
                ],
            )?;
            Ok(p)
        })
        .await
    }

    pub async fn delete_program(&self, id: &str) -> Result<bool> {
        let id = id.to_owned();
        self.with(move |c| {
            let tx = c.transaction()?;
            let n = tx.execute("DELETE FROM programs WHERE id = ?1", [&id])?;
            tx.execute("DELETE FROM runs WHERE program_id = ?1", [&id])?;
            tx.execute("DELETE FROM kv WHERE scope = ?1", [format!("program:{id}")])?;
            tx.commit()?;
            Ok(n > 0)
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
                out.insert(k, serde_json::from_str(&v).unwrap_or(Value::Null));
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
                "INSERT INTO runs (id, program_id, started_at, finished_at, status, error, trigger, trace)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
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
                    r.trace.map(|v| v.to_string())
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
                "SELECT id, program_id, started_at, finished_at, status, error, trigger
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
                "SELECT id, program_id, started_at, finished_at, status, error, trigger, trace
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
        workspace: serde_json::from_str(&ws).unwrap_or(Value::Null),
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn program(id: &str) -> ProgramRecord {
        ProgramRecord {
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
        assert!(s.kv_all("program:a").await.unwrap().is_empty());
        assert!(s.list_programs().await.unwrap().is_empty());
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
            };
            s.save_run(run, 3).await.unwrap();
        }
        let runs = s.list_runs("p", 10).await.unwrap();
        assert_eq!(runs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["r4", "r3", "r2"]);
        assert_eq!(s.get_run("r4").await.unwrap().unwrap().trace, Some(json!([4])));
    }
}
