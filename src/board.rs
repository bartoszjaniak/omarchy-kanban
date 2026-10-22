//! Personal Kanban board: TODO / In Progress / DONE, plus automatic archive.
//!
//! Source of truth for "what we are working on". The user edits it in the
//! Omarchy widget; the AI edits it through the CLI/HTTP API.
use crate::todoist;
use anyhow::{Context, Result, bail};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

/// DONE tasks move to the hidden archive this long after being marked done.
pub const RETENTION_SECS: i64 = 48 * 3600;
pub const BOARD_STATUSES: [&str; 3] = ["todo", "doing", "done"];
pub const ARCHIVED: &str = "archived";

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub notes: String,
    pub status: String,
    pub position: f64,
    pub created_at: i64,
    pub updated_at: i64,
    pub done_at: Option<i64>,
    pub archived_at: Option<i64>,
    pub todoist_id: Option<String>,
    pub todoist_state: Option<String>, // "open" | "closed"
    pub todoist_seen_at: i64,          // epoch of the last settled completion; 0 = none
}

#[derive(Clone, Deserialize)]
pub struct NewTask {
    pub title: String,
    pub notes: Option<String>,
    pub status: Option<String>,
}

#[derive(Clone, Default, Deserialize)]
pub struct TaskPatch {
    pub title: Option<String>,
    pub notes: Option<String>,
    pub status: Option<String>,
    pub position: Option<f64>,
}

fn clean_title(title: &str) -> Result<String> {
    let title = title.trim().to_string();
    if title.is_empty() || title.chars().count() > 200 {
        bail!("Tytuł: 1–200 znaków");
    }
    Ok(title)
}

fn clean_status(status: &str) -> Result<String> {
    if BOARD_STATUSES.contains(&status) {
        Ok(status.to_string())
    } else {
        bail!("Status: todo, doing lub done")
    }
}

pub struct Store {
    conn: Connection,
    pub core_id: String,
}

impl Store {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS tasks(
                 id TEXT PRIMARY KEY,
                 title TEXT NOT NULL,
                 notes TEXT NOT NULL DEFAULT '',
                 status TEXT NOT NULL,
                 position REAL NOT NULL,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL,
                 done_at INTEGER,
                 archived_at INTEGER
             );
             CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )?;
        // Additive migration for boards created before Todoist sync existed.
        let has_todoist: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('tasks') WHERE name='todoist_id'",
            [],
            |r| r.get(0),
        )?;
        if has_todoist == 0 {
            conn.execute_batch(
                "ALTER TABLE tasks ADD COLUMN todoist_id TEXT;
                 ALTER TABLE tasks ADD COLUMN todoist_state TEXT;
                 ALTER TABLE tasks ADD COLUMN todoist_seen_at INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
        conn.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS tasks_todoist
                 ON tasks(todoist_id) WHERE todoist_id IS NOT NULL;",
        )?;
        let core_id = conn
            .query_row("SELECT value FROM meta WHERE key='core_id'", [], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
            .unwrap_or_else(|| {
                let id = uuid::Uuid::new_v4().to_string();
                let _ = conn.execute("INSERT INTO meta VALUES ('core_id', ?1)", [&id]);
                id
            });
        Ok(Self { conn, core_id })
    }

    fn row(row: &rusqlite::Row) -> rusqlite::Result<Task> {
        Ok(Task {
            id: row.get(0)?,
            title: row.get(1)?,
            notes: row.get(2)?,
            status: row.get(3)?,
            position: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
            done_at: row.get(7)?,
            archived_at: row.get(8)?,
            todoist_id: row.get(9)?,
            todoist_state: row.get(10)?,
            todoist_seen_at: row.get(11)?,
        })
    }

    const SELECT: &'static str = "SELECT id,title,notes,status,position,created_at,updated_at,done_at,archived_at,todoist_id,todoist_state,todoist_seen_at FROM tasks";

    pub fn list(&self) -> Result<Vec<Task>> {
        // Active columns first, then archived; deterministic within a column.
        let mut statement = self.conn.prepare(&format!(
            "{} ORDER BY (status='archived'), status, position, created_at",
            Self::SELECT
        ))?;
        let rows = statement.query_map([], Self::row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get(&self, id: &str) -> Result<Option<Task>> {
        Ok(self
            .conn
            .query_row(&format!("{} WHERE id=?1", Self::SELECT), [id], Self::row)
            .optional()?)
    }

    /// Resolve a full id, or a unique id prefix (so the CLI does not need a full UUID).
    pub fn resolve(&self, id: &str) -> Result<String> {
        let id = id.trim();
        if id.is_empty() {
            bail!("Brak identyfikatora zadania");
        }
        if self.get(id)?.is_some() {
            return Ok(id.to_string());
        }
        let mut statement = self
            .conn
            .prepare("SELECT id FROM tasks WHERE id LIKE ?1 || '%' LIMIT 2")?;
        let matches = statement
            .query_map([id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        match matches.as_slice() {
            [only] => Ok(only.clone()),
            [] => bail!("Nie znaleziono zadania {id}"),
            _ => bail!("Niejednoznaczny prefiks {id}; podaj więcej znaków"),
        }
    }

    fn next_position(&self, status: &str) -> Result<f64> {
        let max: Option<f64> = self.conn.query_row(
            "SELECT MAX(position) FROM tasks WHERE status=?1",
            [status],
            |r| r.get(0),
        )?;
        Ok(max.unwrap_or(0.0) + 1.0)
    }

    fn insert(&mut self, task: &Task) -> Result<()> {
        self.conn.execute(
            "INSERT INTO tasks
                 (id,title,notes,status,position,created_at,updated_at,done_at,archived_at,
                  todoist_id,todoist_state,todoist_seen_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                task.id,
                task.title,
                task.notes,
                task.status,
                task.position,
                task.created_at,
                task.updated_at,
                task.done_at,
                task.archived_at,
                task.todoist_id,
                task.todoist_state,
                task.todoist_seen_at
            ],
        )?;
        Ok(())
    }

    pub fn add(&mut self, new: NewTask) -> Result<Task> {
        let title = clean_title(&new.title)?;
        let notes = new.notes.unwrap_or_default();
        if notes.len() > 8192 {
            bail!("Notatki: maksymalnie 8192 znaków");
        }
        let status = clean_status(new.status.as_deref().unwrap_or("todo"))?;
        let at = now();
        let task = Task {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            notes,
            position: self.next_position(&status)?,
            created_at: at,
            updated_at: at,
            done_at: if status == "done" { Some(at) } else { None },
            archived_at: None,
            todoist_id: None,
            todoist_state: None,
            todoist_seen_at: 0,
            status,
        };
        self.insert(&task)?;
        Ok(task)
    }

    /// Like `add`, but links the new card to a Todoist task.
    pub fn add_linked(&mut self, new: NewTask, todoist_id: &str, seen_at: i64) -> Result<Task> {
        if self.is_todoist_linked(todoist_id)? {
            bail!("Zadanie Todoist już jest na tablicy");
        }
        let title = clean_title(&new.title)?;
        let notes = new.notes.unwrap_or_default();
        if notes.len() > 8192 {
            bail!("Notatki: maksymalnie 8192 znaków");
        }
        let status = clean_status(new.status.as_deref().unwrap_or("todo"))?;
        let at = now();
        let task = Task {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            notes,
            position: self.next_position(&status)?,
            created_at: at,
            updated_at: at,
            done_at: if status == "done" { Some(at) } else { None },
            archived_at: None,
            todoist_id: Some(todoist_id.to_string()),
            todoist_state: Some("open".to_string()),
            todoist_seen_at: seen_at,
            status,
        };
        self.insert(&task)?;
        Ok(task)
    }

    pub fn patch(&mut self, id: &str, patch: TaskPatch) -> Result<Task> {
        let id = self.resolve(id)?;
        let current = self
            .get(&id)?
            .with_context(|| format!("Nie znaleziono zadania {id}"))?;
        let title = match &patch.title {
            Some(title) => clean_title(title)?,
            None => current.title.clone(),
        };
        let notes = patch.notes.unwrap_or_else(|| current.notes.clone());
        if notes.len() > 8192 {
            bail!("Notatki: maksymalnie 8192 znaków");
        }
        let status = match &patch.status {
            Some(status) => clean_status(status)?,
            None => current.status.clone(),
        };
        let moved = status != current.status;
        let at = now();
        let position = match patch.position {
            Some(position) if position.is_finite() => position,
            _ if moved => self.next_position(&status)?,
            _ => current.position,
        };
        let task = Task {
            id,
            title,
            notes,
            status: status.clone(),
            position,
            created_at: current.created_at,
            updated_at: at,
            done_at: if status == "done" {
                current.done_at.or(Some(at))
            } else {
                None
            },
            archived_at: None,
            todoist_id: current.todoist_id,
            todoist_state: current.todoist_state,
            todoist_seen_at: current.todoist_seen_at,
        };
        self.conn.execute(
            "UPDATE tasks SET title=?2,notes=?3,status=?4,position=?5,updated_at=?6,done_at=?7,archived_at=NULL WHERE id=?1",
            params![
                task.id,
                task.title,
                task.notes,
                task.status,
                task.position,
                task.updated_at,
                task.done_at
            ],
        )?;
        Ok(task)
    }

    pub fn delete(&mut self, id: &str) -> Result<Task> {
        let id = self.resolve(id)?;
        let task = self
            .get(&id)?
            .with_context(|| format!("Nie znaleziono zadania {id}"))?;
        self.conn.execute("DELETE FROM tasks WHERE id=?1", [&id])?;
        Ok(task)
    }

    /// Move DONE tasks older than the retention window into the hidden archive.
    pub fn maintain(&mut self, at: i64) -> Result<usize> {
        let cutoff = at - RETENTION_SECS;
        let changed = self.conn.execute(
            "UPDATE tasks SET status=?1, archived_at=?2, updated_at=?2
             WHERE status='done' AND done_at IS NOT NULL AND done_at <= ?3",
            params![ARCHIVED, at, cutoff],
        )?;
        Ok(changed)
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
            .optional()?)
    }

    /// Cards linked to Todoist, used as the sync snapshot.
    pub fn todoist_links(&self) -> Result<Vec<TodoistLink>> {
        let mut statement = self.conn.prepare(
            "SELECT id,todoist_id,status,updated_at,todoist_state,todoist_seen_at
             FROM tasks WHERE todoist_id IS NOT NULL",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(TodoistLink {
                task_id: row.get(0)?,
                todoist_id: row.get(1)?,
                status: row.get(2)?,
                updated_at: row.get(3)?,
                state: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                seen_at: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Guards against importing the same Todoist task twice.
    pub fn is_todoist_linked(&self, todoist_id: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM tasks WHERE todoist_id=?1",
                [todoist_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Apply the outcome of one sync cycle in a single transaction.
    pub fn apply_sync(&mut self, writes: &[SyncWrite], at: i64, error: Option<&str>) -> Result<()> {
        let tx = self.conn.transaction()?;
        for write in writes {
            match write {
                SyncWrite::MarkClosed { task_id } => {
                    tx.execute(
                        "UPDATE tasks SET todoist_state='closed' WHERE id=?1",
                        [task_id],
                    )?;
                }
                SyncWrite::MarkOpen { task_id } => {
                    tx.execute(
                        "UPDATE tasks SET todoist_state='open' WHERE id=?1",
                        [task_id],
                    )?;
                }
                SyncWrite::MarkOpenSeen {
                    task_id,
                    completed_at,
                } => {
                    tx.execute(
                        "UPDATE tasks SET todoist_state='open', todoist_seen_at=?1 WHERE id=?2",
                        params![completed_at, task_id],
                    )?;
                }
                SyncWrite::PullDone {
                    task_id,
                    expected_updated_at,
                    done_at,
                    completed_at,
                } => {
                    // Remote state is metadata: always recorded.
                    tx.execute(
                        "UPDATE tasks SET todoist_seen_at=?1, todoist_state='closed' WHERE id=?2",
                        params![completed_at, task_id],
                    )?;
                    // Local status only if the card did not change since the snapshot.
                    tx.execute(
                        "UPDATE tasks SET status='done', done_at=?1, updated_at=?2
                         WHERE id=?3 AND updated_at=?4",
                        params![done_at, at, task_id, expected_updated_at],
                    )?;
                }
                SyncWrite::Unlink { task_id } => {
                    tx.execute(
                        "UPDATE tasks SET todoist_id=NULL, todoist_state=NULL, todoist_seen_at=0
                         WHERE id=?1",
                        [task_id],
                    )?;
                }
            }
        }
        match error {
            None => {
                put_meta(&tx, "todoist_sync_at", &at.to_string())?;
                del_meta(&tx, "todoist_last_error")?;
            }
            Some(message) => put_meta(&tx, "todoist_last_error", message)?,
        }
        tx.commit()?;
        Ok(())
    }
}

fn put_meta(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO meta(key,value) VALUES (?1,?2)
         ON CONFLICT(key) DO UPDATE SET value=?2",
        params![key, value],
    )?;
    Ok(())
}

fn del_meta(conn: &Connection, key: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM meta WHERE key=?1", [key])?;
    Ok(())
}

/// Snapshot row for the sync loop: one Todoist-linked card.
#[derive(Clone, Debug)]
pub struct TodoistLink {
    pub task_id: String,
    pub todoist_id: String,
    pub status: String,
    pub updated_at: i64,
    pub state: String,
    pub seen_at: i64,
}

/// One write-back produced by a sync cycle.
pub enum SyncWrite {
    /// Local card is done/archived: mark the remote state as closed.
    MarkClosed { task_id: String },
    /// Local card is active again: mark the remote state as open.
    MarkOpen { task_id: String },
    /// Remote completion settled against a hand-edited card: record the remote
    /// open state and the settled event, leaving local status/updated_at alone.
    MarkOpenSeen { task_id: String, completed_at: i64 },
    /// Remote completion observed; status written only if `expected_updated_at` still matches.
    PullDone {
        task_id: String,
        expected_updated_at: i64,
        done_at: i64,
        completed_at: i64,
    },
    /// Todoist task is gone: drop the link, keep the card.
    Unlink { task_id: String },
}

#[derive(Clone)]
struct App {
    store: Arc<Mutex<Store>>,
    token: Arc<String>,
    todoist: Option<Arc<todoist::TodoistClient>>,
    sync_gate: Arc<tokio::sync::Mutex<()>>,
}

struct ApiError(anyhow::Error);
impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(error: E) -> Self {
        Self(error.into())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": self.0.to_string()})),
        )
            .into_response()
    }
}
type ApiResult = std::result::Result<Json<Value>, ApiError>;

fn authorized(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.strip_prefix("Bearer ") == Some(token))
}

async fn guard(State(app): State<App>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if !matches!(host.split(':').next(), Some("127.0.0.1" | "localhost")) {
        return (StatusCode::FORBIDDEN, "Loopback Host required").into_response();
    }
    if !authorized(headers, &app.token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Brak tokenu Core"})),
        )
            .into_response();
    }
    let mut response = next.run(request).await;
    for (name, value) in [
        ("cache-control", "no-store"),
        ("referrer-policy", "no-referrer"),
        ("x-content-type-options", "nosniff"),
    ] {
        response.headers_mut().insert(name, value.parse().unwrap());
    }
    response
}

fn counts(tasks: &[Task]) -> Value {
    let mut c = json!({"todo": 0, "doing": 0, "done": 0, "archived": 0});
    for task in tasks {
        if let Some(slot) = c.get_mut(&task.status) {
            let n = slot.as_i64().unwrap_or(0);
            *slot = json!(n + 1);
        }
    }
    c
}

async fn state(State(app): State<App>) -> ApiResult {
    let store = app.store.lock().unwrap();
    let tasks = store.list()?;
    Ok(Json(json!({
        "protocol": 3,
        "mode": "board",
        "core_id": store.core_id,
        "tasks": tasks,
        "counts": counts(&tasks),
        "retention_secs": RETENTION_SECS,
        "now": now(),
    })))
}

async fn add(State(app): State<App>, Json(new): Json<NewTask>) -> ApiResult {
    let task = app.store.lock().unwrap().add(new)?;
    Ok(Json(json!({"task": task})))
}

async fn update(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(patch): Json<TaskPatch>,
) -> ApiResult {
    let task = app.store.lock().unwrap().patch(&id, patch)?;
    Ok(Json(json!({"task": task})))
}

async fn remove(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let task = app.store.lock().unwrap().delete(&id)?;
    Ok(Json(json!({"task": task})))
}

/// Title for an imported card: the remote content cut to the board's
/// 200-character limit, always on a char boundary.
fn import_title(content: &str) -> String {
    content.chars().take(200).collect()
}

/// Notes for an imported card: the remote description, plus a due line.
fn import_note(description: &str, due: Option<&str>) -> String {
    match due {
        Some(due) if description.is_empty() => format!("Termin: {due}"),
        Some(due) => format!("{description}\nTermin: {due}"),
        None => description.to_string(),
    }
}

/// `GET /api/todoist/status` body.
fn status_payload(
    enabled: bool,
    last_sync: Option<&str>,
    last_error: Option<&str>,
    linked: usize,
) -> Value {
    json!({
        "enabled": enabled,
        "last_sync": last_sync.and_then(|value| value.parse::<i64>().ok()),
        "last_error": last_error,
        "linked": linked,
    })
}

/// `POST /api/todoist/sync` body.
fn sync_payload(report: &todoist::SyncReport) -> Value {
    json!({
        "closed": report.closed,
        "reopened": report.reopened,
        "pulled": report.pulled,
        "unlinked": report.unlinked,
        "errors": report.errors,
    })
}

/// The integration is off: no `TODOIST_API_TOKEN` in the service environment.
fn todoist_off() -> ApiError {
    anyhow::anyhow!("Integracja z Todoist wyłączona (brak TODOIST_API_TOKEN)").into()
}

async fn todoist_status(State(app): State<App>) -> ApiResult {
    let store = app.store.lock().unwrap();
    let last_sync = store.meta("todoist_sync_at")?;
    let last_error = store.meta("todoist_last_error")?;
    let linked = store.todoist_links()?.len();
    Ok(Json(status_payload(
        app.todoist.is_some(),
        last_sync.as_deref(),
        last_error.as_deref(),
        linked,
    )))
}

async fn todoist_candidates(State(app): State<App>) -> ApiResult {
    let Some(client) = app.todoist.as_ref() else {
        return Err(todoist_off());
    };
    // Network outside the store lock.
    let remote = client.candidates().await?;
    let store = app.store.lock().unwrap();
    let mut tasks = Vec::new();
    for task in remote {
        if store.is_todoist_linked(&task.id)? {
            continue;
        }
        tasks.push(json!({
            "id": task.id,
            "content": task.content,
            "description": task.description,
            "due": task.due,
            "labels": task.labels,
            "priority": task.priority,
            "project_id": task.project_id,
        }));
    }
    Ok(Json(json!({"tasks": tasks})))
}

/// Upper bound on one import request, so a typo cannot fan out into hundreds
/// of Todoist calls.
const IMPORT_MAX: usize = 50;

#[derive(Deserialize)]
struct ImportRequest {
    ids: Vec<String>,
    #[serde(default)]
    status: Option<String>,
}

async fn todoist_import(State(app): State<App>, Json(request): Json<ImportRequest>) -> ApiResult {
    let Some(client) = app.todoist.as_ref() else {
        return Err(todoist_off());
    };
    if request.ids.len() > IMPORT_MAX {
        return Err(anyhow::anyhow!("Import: maksymalnie {IMPORT_MAX} zadań na żądanie").into());
    }
    let status = clean_status(request.status.as_deref().unwrap_or("todo"))?;
    let at = now();
    let mut imported = Vec::new();
    let mut skipped = Vec::new();
    for id in &request.ids {
        if app.store.lock().unwrap().is_todoist_linked(id)? {
            skipped.push(json!({"id": id, "reason": "already_linked"}));
            continue;
        }
        // A network error aborts the request; a 404 is a plain skip.
        let Some(remote) = client.get_task(id).await? else {
            skipped.push(json!({"id": id, "reason": "not_found"}));
            continue;
        };
        let new = NewTask {
            title: import_title(&remote.content),
            notes: Some(import_note(&remote.description, remote.due.as_deref())),
            status: Some(status.clone()),
        };
        match app.store.lock().unwrap().add_linked(new, id, at) {
            Ok(task) => imported.push(task),
            Err(error) => skipped.push(json!({"id": id, "reason": error.to_string()})),
        }
    }
    Ok(Json(json!({"imported": imported, "skipped": skipped})))
}

async fn todoist_sync(State(app): State<App>) -> ApiResult {
    let Some(client) = app.todoist.as_ref() else {
        return Err(todoist_off());
    };
    let Ok(_guard) = app.sync_gate.try_lock() else {
        return Ok(Json(json!({"busy": true})));
    };
    let report = todoist::sync_once(&app.store, client, now()).await?;
    Ok(Json(sync_payload(&report)))
}

/// Background sync: one cycle per minute, shared single-flight with the
/// `/api/todoist/sync` endpoint. Logs only when a cycle changed something or
/// failed, and never prints the token.
async fn todoist_loop(app: App) {
    let Some(client) = app.todoist.clone() else {
        return;
    };
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let Ok(_guard) = app.sync_gate.try_lock() else {
            continue;
        };
        match todoist::sync_once(&app.store, &client, now()).await {
            Ok(report) => {
                let changed = report.closed + report.reopened + report.pulled + report.unlinked;
                if changed > 0 || !report.errors.is_empty() {
                    eprintln!(
                        "todoist: closed {}, reopened {}, pulled {}, unlinked {}, errors {}",
                        report.closed,
                        report.reopened,
                        report.pulled,
                        report.unlinked,
                        report.errors.len()
                    );
                }
            }
            Err(error) => eprintln!("todoist: {error}"),
        }
    }
}

async fn retention_loop(app: App) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        match app.store.lock().unwrap().maintain(now()) {
            Ok(n) if n > 0 => eprintln!("archived {n} DONE task(s) past {RETENTION_SECS}s"),
            Ok(_) => {}
            Err(e) => eprintln!("retention: {e}"),
        }
    }
}

pub async fn serve(data_dir: PathBuf, port: u16) -> Result<()> {
    crate::private_dir(&data_dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(data_dir.join("core.lock"))?;
    lock.try_lock()
        .context("Core już korzysta z tego katalogu danych")?;
    let token_file = data_dir.join("token");
    let token = if token_file.exists() {
        std::fs::read_to_string(&token_file)?.trim().to_string()
    } else {
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        crate::private_write(&token_file, token.as_bytes())?;
        token
    };
    anyhow::ensure!(token.len() >= 32, "Token Core jest zbyt krótki");
    let mut store = Store::open(&data_dir.join("board.sqlite3"))?;
    store.maintain(now())?;
    let core_id = store.core_id.clone();
    let todoist_client = todoist::TodoistClient::from_env();
    match &todoist_client {
        Some(_) => eprintln!("Todoist: włączone"),
        None => eprintln!("Todoist: wyłączone (brak TODOIST_API_TOKEN)"),
    }
    let app = App {
        store: Arc::new(Mutex::new(store)),
        token: Arc::new(token),
        todoist: todoist_client.map(Arc::new),
        sync_gate: Arc::new(tokio::sync::Mutex::new(())),
    };
    let router = Router::new()
        .route("/api/state", get(state))
        .route("/api/tasks", post(add))
        .route(
            "/api/tasks/{id}",
            axum::routing::patch(update).delete(remove),
        )
        .route("/api/todoist/status", get(todoist_status))
        .route("/api/todoist/candidates", get(todoist_candidates))
        .route("/api/todoist/import", post(todoist_import))
        .route("/api/todoist/sync", post(todoist_sync))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app.clone());
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let bound = listener.local_addr()?.port();
    crate::private_write(
        &data_dir.join("identity.json"),
        &serde_json::to_vec_pretty(
            &json!({"core_id": core_id, "protocol": 3, "mode": "board", "version": env!("CARGO_PKG_VERSION"), "port": bound}),
        )?,
    )?;
    eprintln!(
        "Kanban board {core_id}\nhttp://127.0.0.1:{bound}\nToken file: {}",
        token_file.display()
    );
    let retention = tokio::spawn(retention_loop(app.clone()));
    let todoist_task = app
        .todoist
        .clone()
        .map(|_| tokio::spawn(todoist_loop(app.clone())));
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    retention.abort();
    if let Some(task) = todoist_task {
        task.abort();
    }
    drop(lock);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open(std::path::Path::new(":memory:")).unwrap()
    }

    fn add(store: &mut Store, title: &str, status: &str) -> Task {
        store
            .add(NewTask {
                title: title.into(),
                notes: None,
                status: Some(status.into()),
            })
            .unwrap()
    }

    #[test]
    fn add_move_and_done_at() {
        let mut store = store();
        let task = add(&mut store, "pierwsze", "todo");
        assert_eq!(task.status, "todo");
        assert!(task.done_at.is_none());

        let moved = store
            .patch(
                &task.id,
                TaskPatch {
                    status: Some("done".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(moved.status, "done");
        assert!(moved.done_at.is_some());

        // Moving out of DONE clears the retention clock.
        let back = store
            .patch(
                &task.id,
                TaskPatch {
                    status: Some("doing".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(back.done_at.is_none());
    }

    #[test]
    fn retention_only_archives_old_done() {
        let mut store = store();
        let old = add(&mut store, "stare", "done");
        let fresh = add(&mut store, "świeże", "done");
        let active = add(&mut store, "w toku", "doing");
        let at = now();
        store
            .conn
            .execute(
                "UPDATE tasks SET done_at=?1 WHERE id=?2",
                params![at - RETENTION_SECS - 1, old.id],
            )
            .unwrap();

        assert_eq!(store.maintain(at).unwrap(), 1);
        let list = store.list().unwrap();
        let by = |id: &str| list.iter().find(|t| t.id == id).unwrap().clone();
        assert_eq!(by(&old.id).status, "archived");
        assert!(by(&old.id).archived_at.is_some());
        assert_eq!(by(&fresh.id).status, "done");
        assert_eq!(by(&active.id).status, "doing");
    }

    #[test]
    fn prefix_resolution_and_validation() {
        let mut store = store();
        let task = add(&mut store, "abc", "todo");
        assert_eq!(store.resolve(&task.id[..8]).unwrap(), task.id);
        assert!(store.resolve("zzz").is_err());
        assert!(
            store
                .add(NewTask {
                    title: "  ".into(),
                    notes: None,
                    status: None
                })
                .is_err()
        );
        assert!(
            store
                .add(NewTask {
                    title: "x".into(),
                    notes: None,
                    status: Some("later".into())
                })
                .is_err()
        );
    }

    #[test]
    fn order_is_stable_within_column() {
        let mut store = store();
        add(&mut store, "a", "todo");
        add(&mut store, "b", "todo");
        add(&mut store, "c", "todo");
        let titles: Vec<_> = store.list().unwrap().into_iter().map(|t| t.title).collect();
        assert_eq!(titles, vec!["a", "b", "c"]);
    }

    fn linked(store: &mut Store, title: &str, todoist_id: &str) -> Task {
        store
            .add_linked(
                NewTask {
                    title: title.into(),
                    notes: None,
                    status: Some("todo".into()),
                },
                todoist_id,
                now(),
            )
            .unwrap()
    }

    #[test]
    fn migration_adds_todoist_columns_and_keeps_rows() {
        let path = std::env::temp_dir().join(format!(
            "omarchy-kanban-mig-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        let cleanup = |path: &std::path::Path| {
            let _ = std::fs::remove_file(path);
            let _ = std::fs::remove_file(format!("{}-wal", path.display()));
            let _ = std::fs::remove_file(format!("{}-shm", path.display()));
        };
        {
            // Old schema: no todoist_* columns.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE tasks(
                     id TEXT PRIMARY KEY,
                     title TEXT NOT NULL,
                     notes TEXT NOT NULL DEFAULT '',
                     status TEXT NOT NULL,
                     position REAL NOT NULL,
                     created_at INTEGER NOT NULL,
                     updated_at INTEGER NOT NULL,
                     done_at INTEGER,
                     archived_at INTEGER
                 );
                 CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO tasks VALUES ('old','stary','', 'todo', 1.0, 5, 5, NULL, NULL);",
            )
            .unwrap();
        }

        let mut store = Store::open(&path).unwrap();
        let task = store.get("old").unwrap().unwrap();
        assert_eq!(task.title, "stary");
        assert!(task.todoist_id.is_none());
        assert!(task.todoist_state.is_none());
        assert_eq!(task.todoist_seen_at, 0);

        // add() works on the migrated schema.
        let fresh = add(&mut store, "nowe", "todo");
        assert!(fresh.todoist_id.is_none());
        assert_eq!(fresh.todoist_seen_at, 0);
        assert!(store.todoist_links().unwrap().is_empty());

        drop(store);
        cleanup(&path);
    }

    #[test]
    fn add_linked_sets_link_and_blocks_duplicates() {
        let mut store = store();
        let task = linked(&mut store, "z todoist", "t-1");
        assert_eq!(task.todoist_id.as_deref(), Some("t-1"));
        assert_eq!(task.todoist_state.as_deref(), Some("open"));
        assert!(store.is_todoist_linked("t-1").unwrap());
        assert!(!store.is_todoist_linked("t-2").unwrap());

        let duplicate = store.add_linked(
            NewTask {
                title: "duplikat".into(),
                notes: None,
                status: None,
            },
            "t-1",
            now(),
        );
        assert!(duplicate.is_err());

        let links = store.todoist_links().unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].task_id, task.id);
        assert_eq!(links[0].todoist_id, "t-1");
        assert_eq!(links[0].state, "open");
    }

    #[test]
    fn apply_sync_mark_closed_keeps_updated_at() {
        let mut store = store();
        let task = linked(&mut store, "card", "t-1");
        let before = task.updated_at;

        store
            .apply_sync(
                &[SyncWrite::MarkClosed {
                    task_id: task.id.clone(),
                }],
                now(),
                None,
            )
            .unwrap();

        let after = store.get(&task.id).unwrap().unwrap();
        assert_eq!(after.todoist_state.as_deref(), Some("closed"));
        assert_eq!(after.updated_at, before);
    }

    #[test]
    fn apply_sync_pull_done_respects_compare_and_set() {
        let mut store = store();

        // Matching snapshot: status is applied.
        let applied = linked(&mut store, "applied", "t-a");
        store
            .apply_sync(
                &[SyncWrite::PullDone {
                    task_id: applied.id.clone(),
                    expected_updated_at: applied.updated_at,
                    done_at: 1000,
                    completed_at: 900,
                }],
                1000,
                None,
            )
            .unwrap();
        let card = store.get(&applied.id).unwrap().unwrap();
        assert_eq!(card.status, "done");
        assert_eq!(card.done_at, Some(1000));
        assert_eq!(card.todoist_seen_at, 900);
        assert_eq!(card.todoist_state.as_deref(), Some("closed"));

        // Stale snapshot: status/updated_at untouched, remote state recorded.
        let stale = linked(&mut store, "stale", "t-b");
        store
            .apply_sync(
                &[SyncWrite::PullDone {
                    task_id: stale.id.clone(),
                    expected_updated_at: stale.updated_at - 1,
                    done_at: 2000,
                    completed_at: 1900,
                }],
                2000,
                None,
            )
            .unwrap();
        let card = store.get(&stale.id).unwrap().unwrap();
        assert_eq!(card.status, "todo");
        assert_eq!(card.updated_at, stale.updated_at);
        assert_eq!(card.todoist_seen_at, 1900);
        assert_eq!(card.todoist_state.as_deref(), Some("closed"));
    }

    #[test]
    fn apply_sync_mark_open_seen_keeps_local_card() {
        let mut store = store();
        let task = linked(&mut store, "card", "t-1");
        let before = task.updated_at;

        store
            .apply_sync(
                &[SyncWrite::MarkOpenSeen {
                    task_id: task.id.clone(),
                    completed_at: 1234,
                }],
                now(),
                None,
            )
            .unwrap();

        let card = store.get(&task.id).unwrap().unwrap();
        assert_eq!(card.todoist_state.as_deref(), Some("open"));
        assert_eq!(card.todoist_seen_at, 1234);
        assert_eq!(card.updated_at, before);
        assert_eq!(card.status, "todo");
    }

    #[test]
    fn apply_sync_unlink_clears_link() {
        let mut store = store();
        let task = linked(&mut store, "card", "t-1");
        store
            .apply_sync(
                &[SyncWrite::Unlink {
                    task_id: task.id.clone(),
                }],
                now(),
                None,
            )
            .unwrap();
        let card = store.get(&task.id).unwrap().unwrap();
        assert!(card.todoist_id.is_none());
        assert!(card.todoist_state.is_none());
        assert_eq!(card.todoist_seen_at, 0);
        assert!(!store.is_todoist_linked("t-1").unwrap());
    }

    #[test]
    fn apply_sync_records_meta_by_outcome() {
        let mut store = store();
        let task = linked(&mut store, "card", "t-1");

        store
            .apply_sync(
                &[SyncWrite::MarkOpen {
                    task_id: task.id.clone(),
                }],
                4242,
                Some("boom"),
            )
            .unwrap();
        assert_eq!(store.meta("todoist_sync_at").unwrap(), None);
        assert_eq!(
            store.meta("todoist_last_error").unwrap().as_deref(),
            Some("boom")
        );

        store
            .apply_sync(
                &[SyncWrite::MarkClosed {
                    task_id: task.id.clone(),
                }],
                7777,
                None,
            )
            .unwrap();
        assert_eq!(
            store.meta("todoist_sync_at").unwrap().as_deref(),
            Some("7777")
        );
        assert_eq!(store.meta("todoist_last_error").unwrap(), None);
    }

    #[test]
    fn import_note_adds_due_line() {
        assert_eq!(import_note("opis", None), "opis");
        assert_eq!(
            import_note("opis", Some("2026-10-22")),
            "opis\nTermin: 2026-10-22"
        );
        assert_eq!(import_note("", Some("dziś")), "Termin: dziś");
        assert_eq!(import_note("", None), "");
    }

    #[test]
    fn import_title_cuts_at_200_chars() {
        assert_eq!(import_title("krótki"), "krótki");
        let long = "ż".repeat(250);
        let cut = import_title(&long);
        assert_eq!(cut.chars().count(), 200);
        assert!(cut.chars().all(|c| c == 'ż'));
        // Exactly 200 is kept.
        assert_eq!(import_title(&"a".repeat(200)).len(), 200);
    }

    #[test]
    fn status_payload_shapes_fields() {
        assert_eq!(
            status_payload(true, Some("42"), Some("boom"), 3),
            json!({"enabled": true, "last_sync": 42, "last_error": "boom", "linked": 3})
        );
        // Unparseable or missing last_sync becomes null.
        assert_eq!(
            status_payload(false, None, None, 0),
            json!({"enabled": false, "last_sync": null, "last_error": null, "linked": 0})
        );
        assert_eq!(
            status_payload(true, Some("not-a-number"), None, 1)["last_sync"],
            Value::Null
        );
    }

    #[test]
    fn sync_payload_maps_report() {
        let report = todoist::SyncReport {
            closed: 1,
            reopened: 2,
            pulled: 3,
            unlinked: 4,
            errors: vec!["Zadanie Todoist t-1 nie istnieje".into()],
        };
        assert_eq!(
            sync_payload(&report),
            json!({
                "closed": 1,
                "reopened": 2,
                "pulled": 3,
                "unlinked": 4,
                "errors": ["Zadanie Todoist t-1 nie istnieje"],
            })
        );
        assert_eq!(
            sync_payload(&todoist::SyncReport::default()),
            json!({"closed": 0, "reopened": 0, "pulled": 0, "unlinked": 0, "errors": []})
        );
    }
}
