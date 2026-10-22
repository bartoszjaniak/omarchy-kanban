//! Personal Kanban board: TODO / In Progress / DONE, plus automatic archive.
//!
//! Source of truth for "what we are working on". The user edits it in the
//! Omarchy widget; the AI edits it through the CLI/HTTP API.
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
        })
    }

    const SELECT: &'static str = "SELECT id,title,notes,status,position,created_at,updated_at,done_at,archived_at FROM tasks";

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
            status,
        };
        self.conn.execute(
            "INSERT INTO tasks VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                task.id,
                task.title,
                task.notes,
                task.status,
                task.position,
                task.created_at,
                task.updated_at,
                task.done_at,
                task.archived_at
            ],
        )?;
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
}

#[derive(Clone)]
struct App {
    store: Arc<Mutex<Store>>,
    token: Arc<String>,
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
    let app = App {
        store: Arc::new(Mutex::new(store)),
        token: Arc::new(token),
    };
    let router = Router::new()
        .route("/api/state", get(state))
        .route("/api/tasks", post(add))
        .route("/api/tasks/{id}", axum::routing::patch(update).delete(remove))
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
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    retention.abort();
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
        assert!(store.add(NewTask { title: "  ".into(), notes: None, status: None }).is_err());
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
        let titles: Vec<_> = store
            .list()
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect();
        assert_eq!(titles, vec!["a", "b", "c"]);
    }
}
