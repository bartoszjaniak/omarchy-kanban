//! Todoist REST API v1 client: candidate tasks, single task, completions and
//! close/reopen. Network errors never include the token. No new dependencies.

use crate::board::{self, SyncWrite};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::sync::Mutex;
use std::time::Duration;

const DEFAULT_BASE_URL: &str = "https://api.todoist.com/api/v1";
/// Todoist caps `limit` at 200.
const PAGE_LIMIT: u32 = 200;
/// Guard against a provider that never stops handing out cursors.
const MAX_PAGES: usize = 50;

/// A task as delivered by Todoist, reduced to the fields the board needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteTask {
    pub id: String,
    pub content: String,
    pub description: String,
    pub due: Option<String>,
    pub labels: Vec<String>,
    pub priority: i64,
    pub project_id: String,
}

/// Result of a state-changing request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    /// 2xx: the state change was applied.
    Applied,
    /// 404: the task no longer exists (unlink on the board).
    Gone,
    /// 400: the task is already in the requested state, or cannot transition.
    Already,
}

pub struct TodoistClient {
    base_url: String,
    token: String,
    client: reqwest::Client,
}

impl TodoistClient {
    /// Build a client. The token must be non-empty; the base URL has trailing
    /// slashes trimmed so paths can be appended with a single `/`.
    pub fn new(base_url: &str, token: &str) -> Result<Self> {
        let token = token.trim();
        if token.is_empty() {
            bail!("TODOIST_API_TOKEN is empty");
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .no_proxy()
            .build()?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
            client,
        })
    }

    /// Client from the environment, or `None` when the integration is off
    /// (missing, blank, or implausibly short `TODOIST_API_TOKEN`).
    pub fn from_env() -> Option<Self> {
        let token = std::env::var("TODOIST_API_TOKEN").ok()?;
        if token.trim().len() < 20 {
            return None;
        }
        let base =
            std::env::var("TODOIST_API_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
        Self::new(&base, &token).ok()
    }

    /// Tasks matching `today | overdue`, all pages.
    pub async fn candidates(&self) -> Result<Vec<RemoteTask>> {
        let query = [
            ("query", "today | overdue".to_string()),
            ("limit", PAGE_LIMIT.to_string()),
        ];
        self.paginate("/tasks/filter", &query, "results", remote_task)
            .await
    }

    /// Authoritative data for one task; `None` on 404.
    pub async fn get_task(&self, id: &str) -> Result<Option<RemoteTask>> {
        let path = format!("/tasks/{id}");
        let response = self
            .client
            .get(self.url(&path))
            .bearer_auth(&self.token)
            .send()
            .await
            .with_context(|| format!("Todoist request failed: GET {path}"))?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            bail!("{}", error_message(status, status_body(response).await));
        }
        let body: Value = response
            .json()
            .await
            .with_context(|| format!("Todoist invalid JSON: GET {path}"))?;
        Ok(remote_task(&body))
    }

    /// Completions in `[since, until]` as `(task_id, completed_at epoch)`, all
    /// pages. Items without a parseable `completed_at` are skipped.
    pub async fn completed_since(&self, since: i64, until: i64) -> Result<Vec<(String, i64)>> {
        let query = [
            ("since", iso8601(since)),
            ("until", iso8601(until)),
            ("limit", PAGE_LIMIT.to_string()),
        ];
        self.paginate(
            "/tasks/completed/by_completion_date",
            &query,
            "items",
            |value| {
                let id = value.get("id")?.as_str()?.to_string();
                let epoch = parse_iso8601(value.get("completed_at")?.as_str()?)?;
                Some((id, epoch))
            },
        )
        .await
    }

    /// Close the task; see [`WriteOutcome`].
    pub async fn close(&self, id: &str) -> Result<WriteOutcome> {
        self.write(id, "close").await
    }

    /// Reopen the task; see [`WriteOutcome`].
    pub async fn reopen(&self, id: &str) -> Result<WriteOutcome> {
        self.write(id, "reopen").await
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        let response = self
            .client
            .get(self.url(path))
            .query(query)
            .bearer_auth(&self.token)
            .send()
            .await
            .with_context(|| format!("Todoist request failed: GET {path}"))?;
        let status = response.status();
        if !status.is_success() {
            bail!("{}", error_message(status, status_body(response).await));
        }
        response
            .json()
            .await
            .with_context(|| format!("Todoist invalid JSON: GET {path}"))
    }

    async fn write(&self, id: &str, action: &str) -> Result<WriteOutcome> {
        let path = format!("/tasks/{id}/{action}");
        let response = self
            .client
            .post(self.url(&path))
            .bearer_auth(&self.token)
            .send()
            .await
            .with_context(|| format!("Todoist request failed: POST {path}"))?;
        let status = response.status();
        if status.is_success() {
            return Ok(WriteOutcome::Applied);
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(WriteOutcome::Gone);
        }
        if status == reqwest::StatusCode::BAD_REQUEST {
            return Ok(WriteOutcome::Already);
        }
        bail!("{}", error_message(status, status_body(response).await));
    }

    /// Follow `next_cursor` until `null`, collecting mapped `array_key` items.
    async fn paginate<T>(
        &self,
        path: &str,
        base_query: &[(&str, String)],
        array_key: &str,
        map: impl Fn(&Value) -> Option<T>,
    ) -> Result<Vec<T>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut query = base_query.to_vec();
            if let Some(cursor) = &cursor {
                query.push(("cursor", cursor.clone()));
            }
            let body = self.get(path, &query).await?;
            if let Some(items) = body.get(array_key).and_then(Value::as_array) {
                out.extend(items.iter().filter_map(&map));
            }
            cursor = body
                .get("next_cursor")
                .and_then(Value::as_str)
                .filter(|cursor| !cursor.is_empty())
                .map(str::to_string);
            if cursor.is_none() {
                return Ok(out);
            }
        }
        bail!("Todoist pagination exceeded {MAX_PAGES} pages")
    }
}

fn remote_task(value: &Value) -> Option<RemoteTask> {
    let id = value.get("id")?.as_str()?.to_string();
    Some(RemoteTask {
        id,
        content: string_field(value, "content"),
        description: string_field(value, "description"),
        due: due_of(value),
        labels: value
            .get("labels")
            .and_then(Value::as_array)
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        priority: value.get("priority").and_then(Value::as_i64).unwrap_or(0),
        project_id: string_field(value, "project_id"),
    })
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `due` is an object; prefer its human `string`, fall back to `date`.
fn due_of(value: &Value) -> Option<String> {
    let due = value.get("due")?;
    if due.is_null() {
        return None;
    }
    due.get("string")
        .and_then(Value::as_str)
        .or_else(|| due.get("date").and_then(Value::as_str))
        .map(str::to_string)
}

/// Read the body only on error; never includes the token or headers.
async fn status_body(response: reqwest::Response) -> String {
    response.text().await.unwrap_or_default()
}

fn error_message(status: reqwest::StatusCode, body: String) -> String {
    match serde_json::from_str::<Value>(&body).ok().and_then(|value| {
        value
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_string)
    }) {
        Some(detail) => format!("Todoist {status}: {detail}"),
        None => format!("Todoist {status}"),
    }
}

/// Epoch seconds → `YYYY-MM-DDTHH:MM:SSZ` (UTC).
pub fn iso8601(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// `YYYY-MM-DDTHH:MM:SSZ` → epoch seconds. Optional fractional seconds are
/// accepted and ignored. Non-canonical input yields `None`.
pub fn parse_iso8601(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    let year: i64 = text.get(0..4)?.parse().ok()?;
    let month: u32 = text.get(5..7)?.parse().ok()?;
    let day: u32 = text.get(8..10)?.parse().ok()?;
    let hour: i64 = text.get(11..13)?.parse().ok()?;
    let minute: i64 = text.get(14..16)?.parse().ok()?;
    let second: i64 = text.get(17..19)?.parse().ok()?;
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let tail = &text[19..];
    let tail = match tail.strip_prefix('.') {
        Some(fraction) => {
            let digits = fraction.strip_suffix('Z')?;
            if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            "Z"
        }
        None => tail,
    };
    if tail != "Z"
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

/// Howard Hinnant's `days_from_civil`: days since 1970-01-01.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = i64::from(month);
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Howard Hinnant's `civil_from_days`: inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (year + i64::from(month <= 2), month, day)
}

/// Outcome counters and messages of one sync cycle.
#[derive(Debug, Default)]
pub struct SyncReport {
    pub closed: usize,
    pub reopened: usize,
    pub pulled: usize,
    pub unlinked: usize,
    pub errors: Vec<String>,
}

/// Seconds subtracted from `last_sync` to widen the completion query window.
const PULL_BACKOFF: i64 = 300;
/// Query window used before the first successful sync.
const PULL_FALLBACK: i64 = 7 * 86_400;
/// Todoist rejects completion ranges wider than three months.
const PULL_MAX_WINDOW: i64 = 89 * 86_400;
/// Stored `todoist_last_error` is truncated to this many characters.
const ERROR_MAX: usize = 500;

/// Run one sync cycle: push local state, then pull remote completions, then a
/// single write. Network happens outside the store lock.
pub async fn sync_once(
    store: &Mutex<board::Store>,
    client: &TodoistClient,
    at: i64,
) -> Result<SyncReport> {
    // 1. Snapshot under one short lock; the guard must not survive an await.
    let (links, last_sync) = {
        let store = store.lock().unwrap();
        let links = store.todoist_links()?;
        let last_sync = store
            .meta("todoist_sync_at")?
            .and_then(|value| value.parse::<i64>().ok());
        (links, last_sync)
    };

    let mut report = SyncReport::default();
    if links.is_empty() {
        // Healthy cycle with nothing to do: record the tick, skip the network.
        store.lock().unwrap().apply_sync(&[], at, None)?;
        return Ok(report);
    }

    let mut writes = Vec::new();

    // 2. Push: one state change per card at most.
    for link in &links {
        let closing = matches!(link.status.as_str(), "done" | "archived") && link.state != "closed";
        let reopening = matches!(link.status.as_str(), "todo" | "doing") && link.state == "closed";
        if !closing && !reopening {
            continue;
        }
        let outcome = if closing {
            client.close(&link.todoist_id).await
        } else {
            client.reopen(&link.todoist_id).await
        };
        match outcome {
            Ok(WriteOutcome::Applied) => {
                if closing {
                    writes.push(SyncWrite::MarkClosed {
                        task_id: link.task_id.clone(),
                    });
                    report.closed += 1;
                } else {
                    writes.push(SyncWrite::MarkOpen {
                        task_id: link.task_id.clone(),
                    });
                    report.reopened += 1;
                }
            }
            // 400: already in the requested state; record it without counting.
            Ok(WriteOutcome::Already) => writes.push(if closing {
                SyncWrite::MarkClosed {
                    task_id: link.task_id.clone(),
                }
            } else {
                SyncWrite::MarkOpen {
                    task_id: link.task_id.clone(),
                }
            }),
            Ok(WriteOutcome::Gone) => {
                writes.push(SyncWrite::Unlink {
                    task_id: link.task_id.clone(),
                });
                report.unlinked += 1;
                report
                    .errors
                    .push(format!("Zadanie Todoist {} nie istnieje", link.todoist_id));
            }
            // A single card's failure must not abort the cycle.
            Err(error) => report.errors.push(error.to_string()),
        }
    }

    // 3. Pull: only when at least one linked card is still active.
    let has_active = links
        .iter()
        .any(|link| matches!(link.status.as_str(), "todo" | "doing"));
    if has_active {
        let since = last_sync
            .map(|last| last - PULL_BACKOFF)
            .unwrap_or(at - PULL_FALLBACK)
            // A sync disabled for months must not ask for an impossible window.
            .max(at - PULL_MAX_WINDOW);
        match client.completed_since(since, at).await {
            Ok(completed) => {
                for (todoist_id, completed_at) in completed {
                    let Some(link) = links.iter().find(|link| {
                        link.todoist_id == todoist_id
                            && matches!(link.status.as_str(), "todo" | "doing")
                    }) else {
                        continue;
                    };
                    // Already settled in an earlier window.
                    if completed_at <= link.seen_at {
                        continue;
                    }
                    // Untouched: no prior sync, or edited only before it.
                    let untouched = last_sync.is_none_or(|last| link.updated_at < last);
                    if untouched {
                        writes.push(SyncWrite::PullDone {
                            task_id: link.task_id.clone(),
                            expected_updated_at: link.updated_at,
                            done_at: at,
                            completed_at,
                        });
                        report.pulled += 1;
                    } else {
                        // Manual edit wins: push the local state and settle the event.
                        match client.reopen(&todoist_id).await {
                            Ok(WriteOutcome::Applied) | Ok(WriteOutcome::Already) => {
                                writes.push(SyncWrite::MarkOpenSeen {
                                    task_id: link.task_id.clone(),
                                    completed_at,
                                });
                            }
                            Ok(WriteOutcome::Gone) => {
                                writes.push(SyncWrite::Unlink {
                                    task_id: link.task_id.clone(),
                                });
                                report.unlinked += 1;
                                report
                                    .errors
                                    .push(format!("Zadanie Todoist {todoist_id} nie istnieje"));
                            }
                            Err(error) => report.errors.push(error.to_string()),
                        }
                    }
                }
            }
            Err(error) => report.errors.push(error.to_string()),
        }
    }

    // 4. Single write: only a clean cycle advances `todoist_sync_at`.
    let error = if report.errors.is_empty() {
        None
    } else {
        let joined = report.errors.join("; ");
        Some(joined.chars().take(ERROR_MAX).collect::<String>())
    };
    store
        .lock()
        .unwrap()
        .apply_sync(&writes, at, error.as_deref())?;

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::{Path, Query, Request, State},
        http::{StatusCode, header},
        middleware::{self, Next},
        response::{IntoResponse, Response},
        routing::{get, post},
    };
    use serde_json::json;
    use std::collections::HashMap;

    use crate::board::{self, NewTask, Store, TaskPatch};
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    const TOKEN: &str = "test-token";

    /// Rejects anything that is not `Authorization: Bearer test-token`.
    async fn require_auth(request: Request, next: Next) -> Response {
        let authorized = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            == Some("Bearer test-token");
        if authorized {
            next.run(request).await
        } else {
            StatusCode::UNAUTHORIZED.into_response()
        }
    }

    /// Serve `router` on a random loopback port; returns the base URL.
    async fn spawn(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        format!("http://{addr}")
    }

    fn guarded(router: Router) -> Router {
        router.layer(middleware::from_fn(require_auth))
    }

    fn client(base_url: &str) -> TodoistClient {
        TodoistClient::new(base_url, TOKEN).unwrap()
    }

    fn task_json(id: &str) -> Value {
        json!({
            "id": id,
            "content": format!("task {id}"),
            "description": "desc",
            "due": {"string": "today", "date": "2026-10-22"},
            "labels": ["home", "pc"],
            "priority": 3,
            "project_id": "proj-1"
        })
    }

    #[test]
    fn iso8601_epochs() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1_709_164_800), "2024-02-29T00:00:00Z");
        assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso8601("2024-02-29T00:00:00Z"), Some(1_709_164_800));
    }

    #[test]
    fn iso8601_roundtrip() {
        for day in (0..=47_481).step_by(97) {
            let epoch = day * 86_400 + 3661;
            let text = iso8601(epoch);
            assert_eq!(parse_iso8601(&text), Some(epoch), "{text}");
        }
    }

    #[test]
    fn parse_iso8601_fractional_and_invalid() {
        assert_eq!(
            parse_iso8601("2026-10-22T10:00:00.123Z"),
            parse_iso8601("2026-10-22T10:00:00Z")
        );
        assert!(parse_iso8601("2026-10-22T10:00:00").is_none());
        assert!(parse_iso8601("2026-13-01T00:00:00Z").is_none());
        assert!(parse_iso8601("garbage").is_none());
    }

    #[test]
    fn new_rejects_empty_token() {
        assert!(TodoistClient::new("http://127.0.0.1:1", "   ").is_err());
    }

    #[tokio::test]
    async fn candidates_collects_two_pages() {
        async fn handler(Query(params): Query<HashMap<String, String>>) -> Json<Value> {
            assert_eq!(
                params.get("query").map(String::as_str),
                Some("today | overdue")
            );
            assert_eq!(params.get("limit").map(String::as_str), Some("200"));
            match params.get("cursor").map(String::as_str) {
                None => Json(json!({"results": [task_json("1")], "next_cursor": "page-2"})),
                Some("page-2") => Json(json!({"results": [task_json("2")], "next_cursor": null})),
                Some(other) => panic!("unexpected cursor: {other}"),
            }
        }

        let base = spawn(guarded(Router::new().route("/tasks/filter", get(handler)))).await;
        let tasks = client(&base).candidates().await.unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, "1");
        assert_eq!(tasks[1].content, "task 2");
        assert_eq!(tasks[0].due.as_deref(), Some("today"));
        assert_eq!(tasks[0].labels, vec!["home", "pc"]);
        assert_eq!(tasks[0].priority, 3);
        assert_eq!(tasks[0].project_id, "proj-1");
    }

    #[tokio::test]
    async fn get_task_maps_and_404_is_none() {
        async fn handler(Path(id): Path<String>) -> Response {
            if id == "missing" {
                StatusCode::NOT_FOUND.into_response()
            } else {
                Json(task_json(&id)).into_response()
            }
        }

        let base = spawn(guarded(Router::new().route("/tasks/{id}", get(handler)))).await;
        let client = client(&base);
        let task = client.get_task("42").await.unwrap().unwrap();
        assert_eq!(task.id, "42");
        assert_eq!(task.description, "desc");
        assert!(client.get_task("missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn close_maps_statuses() {
        async fn handler(Path(id): Path<String>) -> StatusCode {
            match id.as_str() {
                "applied" => StatusCode::NO_CONTENT,
                "already" => StatusCode::BAD_REQUEST,
                "gone" => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            }
        }

        let base = spawn(guarded(
            Router::new().route("/tasks/{id}/close", post(handler)),
        ))
        .await;
        let client = client(&base);
        assert_eq!(
            client.close("applied").await.unwrap(),
            WriteOutcome::Applied
        );
        assert_eq!(
            client.close("already").await.unwrap(),
            WriteOutcome::Already
        );
        assert_eq!(client.close("gone").await.unwrap(), WriteOutcome::Gone);
        assert!(client.close("boom").await.is_err());
    }

    #[tokio::test]
    async fn reopen_maps_statuses() {
        async fn handler(Path(id): Path<String>) -> StatusCode {
            match id.as_str() {
                "ok" => StatusCode::OK,
                "already" => StatusCode::BAD_REQUEST,
                "gone" => StatusCode::NOT_FOUND,
                _ => StatusCode::TOO_MANY_REQUESTS,
            }
        }

        let base = spawn(guarded(
            Router::new().route("/tasks/{id}/reopen", post(handler)),
        ))
        .await;
        let client = client(&base);
        assert_eq!(client.reopen("ok").await.unwrap(), WriteOutcome::Applied);
        assert_eq!(
            client.reopen("already").await.unwrap(),
            WriteOutcome::Already
        );
        assert_eq!(client.reopen("gone").await.unwrap(), WriteOutcome::Gone);
        assert!(client.reopen("limited").await.is_err());
    }

    #[tokio::test]
    async fn completed_since_maps_and_paginates() {
        async fn handler(Query(params): Query<HashMap<String, String>>) -> Json<Value> {
            assert_eq!(params.get("limit").map(String::as_str), Some("200"));
            assert_eq!(
                params.get("since").map(String::as_str),
                Some("1970-01-01T00:00:00Z")
            );
            assert_eq!(
                params.get("until").map(String::as_str),
                Some("2026-10-22T10:00:00Z")
            );
            match params.get("cursor").map(String::as_str) {
                None => Json(json!({
                    "items": [{"id": "a", "completed_at": "2026-10-22T09:00:00Z"}],
                    "next_cursor": "p2"
                })),
                Some("p2") => Json(json!({
                    "items": [
                        {"id": "b", "completed_at": null},
                        {"id": "c", "completed_at": "2026-10-22T09:30:00.500Z"}
                    ],
                    "next_cursor": null
                })),
                Some(other) => panic!("unexpected cursor: {other}"),
            }
        }

        let base = spawn(guarded(
            Router::new().route("/tasks/completed/by_completion_date", get(handler)),
        ))
        .await;
        let since = 0;
        let until = parse_iso8601("2026-10-22T10:00:00Z").unwrap();
        let done = client(&base).completed_since(since, until).await.unwrap();
        assert_eq!(
            done,
            vec![
                (
                    "a".to_string(),
                    parse_iso8601("2026-10-22T09:00:00Z").unwrap()
                ),
                (
                    "c".to_string(),
                    parse_iso8601("2026-10-22T09:30:00.500Z").unwrap()
                ),
            ]
        );
    }

    #[tokio::test]
    async fn wrong_token_returns_error_not_panic() {
        let base = spawn(guarded(
            Router::new().route("/tasks/{id}", get(|| async { "ok" })),
        ))
        .await;
        let client = TodoistClient::new(&base, "wrong-token").unwrap();
        let error = client.get_task("42").await.unwrap_err().to_string();
        assert!(error.contains("401"), "{error}");
        assert!(!error.contains("wrong-token"), "leaked token: {error}");
    }

    // --- sync_once -------------------------------------------------------

    /// Configurable Todoist stub: records calls, serves completions, and can
    /// inject error statuses or a mid-cycle hand edit of the board DB.
    #[derive(Default)]
    struct Stub {
        active: Mutex<HashSet<String>>,
        completions: Mutex<Vec<(String, i64)>>,
        calls: Mutex<HashMap<String, usize>>,
        close_status: Mutex<HashMap<String, StatusCode>>,
        reopen_status: Mutex<HashMap<String, StatusCode>>,
        completed_status: Mutex<Option<StatusCode>>,
        db_path: Mutex<Option<String>>,
        touch: Mutex<Option<(String, i64)>>,
    }

    impl Stub {
        fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        async fn serve(self: &Arc<Self>) -> String {
            spawn(
                Router::new()
                    .route("/tasks/{id}/close", post(stub_close))
                    .route("/tasks/{id}/reopen", post(stub_reopen))
                    .route("/tasks/completed/by_completion_date", get(stub_completed))
                    .layer(middleware::from_fn(require_auth))
                    .with_state(self.clone()),
            )
            .await
        }

        fn bump(&self, key: &str) {
            *self
                .calls
                .lock()
                .unwrap()
                .entry(key.to_string())
                .or_default() += 1;
        }

        fn calls(&self, key: &str) -> usize {
            self.calls.lock().unwrap().get(key).copied().unwrap_or(0)
        }

        fn total(&self) -> usize {
            self.calls.lock().unwrap().values().copied().sum()
        }

        fn is_active(&self, id: &str) -> bool {
            self.active.lock().unwrap().contains(id)
        }

        fn set_close(&self, id: &str, status: StatusCode) {
            self.close_status
                .lock()
                .unwrap()
                .insert(id.to_string(), status);
        }

        fn set_reopen(&self, id: &str, status: StatusCode) {
            self.reopen_status
                .lock()
                .unwrap()
                .insert(id.to_string(), status);
        }

        fn complete(&self, id: &str, at: i64) {
            self.completions.lock().unwrap().push((id.to_string(), at));
        }

        /// Apply a raw `updated_at` change while a completion request is in flight.
        fn touch_during_pull(&self, db_path: &std::path::Path, task_id: &str, updated_at: i64) {
            *self.db_path.lock().unwrap() = Some(db_path.display().to_string());
            *self.touch.lock().unwrap() = Some((task_id.to_string(), updated_at));
        }
    }

    async fn stub_close(State(stub): State<Arc<Stub>>, Path(id): Path<String>) -> StatusCode {
        stub.bump(&format!("close:{id}"));
        if let Some(status) = stub.close_status.lock().unwrap().get(&id).copied() {
            return status;
        }
        stub.active.lock().unwrap().remove(&id);
        StatusCode::NO_CONTENT
    }

    async fn stub_reopen(State(stub): State<Arc<Stub>>, Path(id): Path<String>) -> StatusCode {
        stub.bump(&format!("reopen:{id}"));
        if let Some(status) = stub.reopen_status.lock().unwrap().get(&id).copied() {
            return status;
        }
        stub.active.lock().unwrap().insert(id);
        StatusCode::NO_CONTENT
    }

    async fn stub_completed(
        State(stub): State<Arc<Stub>>,
        Query(params): Query<HashMap<String, String>>,
    ) -> Response {
        stub.bump("completed");
        // The pull window is a real contract: a missing or malformed `since`/
        // `until`, or an empty/inverted range, must fail loudly so window
        // arithmetic bugs (PULL_BACKOFF/PULL_FALLBACK/PULL_MAX_WINDOW) surface.
        let (Some(since), Some(until)) = (
            params.get("since").and_then(|value| parse_iso8601(value)),
            params.get("until").and_then(|value| parse_iso8601(value)),
        ) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        if since >= until {
            return StatusCode::BAD_REQUEST.into_response();
        }
        if let Some(status) = *stub.completed_status.lock().unwrap() {
            return status.into_response();
        }
        // Simulate a manual edit landing while the cycle is over the network.
        if let (Some(path), Some((task_id, updated_at))) = (
            stub.db_path.lock().unwrap().clone(),
            stub.touch.lock().unwrap().clone(),
        ) {
            let conn = rusqlite::Connection::open(path).unwrap();
            conn.execute(
                "UPDATE tasks SET updated_at=?1 WHERE id=?2",
                rusqlite::params![updated_at, task_id],
            )
            .unwrap();
        }
        let items: Vec<Value> = stub
            .completions
            .lock()
            .unwrap()
            .iter()
            .map(|(id, at)| json!({"id": id, "completed_at": iso8601(*at)}))
            .collect();
        Json(json!({"items": items, "next_cursor": null})).into_response()
    }

    fn mem_store() -> Mutex<Store> {
        Mutex::new(Store::open(std::path::Path::new(":memory:")).unwrap())
    }

    fn temp_db() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "omarchy-kanban-sync-{}.sqlite3",
            uuid::Uuid::new_v4()
        ))
    }

    fn cleanup(path: &std::path::Path) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    /// Link a card with `todoist_seen_at = 0`, so any completion counts as new.
    fn link(store: &Mutex<Store>, title: &str, status: &str, todoist_id: &str) -> String {
        store
            .lock()
            .unwrap()
            .add_linked(
                NewTask {
                    title: title.to_string(),
                    notes: None,
                    status: Some(status.to_string()),
                },
                todoist_id,
                0,
            )
            .unwrap()
            .id
    }

    fn card(store: &Mutex<Store>, id: &str) -> board::Task {
        store.lock().unwrap().get(id).unwrap().unwrap()
    }

    fn meta(store: &Mutex<Store>, key: &str) -> Option<String> {
        store.lock().unwrap().meta(key).unwrap()
    }

    fn move_card(store: &Mutex<Store>, id: &str, status: &str) {
        store
            .lock()
            .unwrap()
            .patch(
                id,
                TaskPatch {
                    status: Some(status.to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
    }

    #[tokio::test]
    async fn import_close_reopen_three_cycles() {
        let stub = Stub::new();
        let client = client(&stub.serve().await);
        let store = mem_store();
        let id = link(&store, "karta", "todo", "t-1");

        // Cycle 1: nothing to push.
        let report = sync_once(&store, &client, 1_000).await.unwrap();
        assert_eq!(report.closed, 0);
        assert_eq!(stub.calls("close:t-1"), 0);
        assert_eq!(card(&store, &id).todoist_state.as_deref(), Some("open"));

        // Card → DONE: exactly one close.
        move_card(&store, &id, "done");
        let report = sync_once(&store, &client, 2_000).await.unwrap();
        assert_eq!(report.closed, 1);
        assert_eq!(stub.calls("close:t-1"), 1);
        assert!(!stub.is_active("t-1"));
        assert_eq!(card(&store, &id).todoist_state.as_deref(), Some("closed"));

        // DONE → doing: exactly one reopen.
        move_card(&store, &id, "doing");
        let report = sync_once(&store, &client, 3_000).await.unwrap();
        assert_eq!(report.reopened, 1);
        assert_eq!(stub.calls("reopen:t-1"), 1);
        assert!(stub.is_active("t-1"));
        assert_eq!(card(&store, &id).todoist_state.as_deref(), Some("open"));
    }

    #[tokio::test]
    async fn pull_completion_marks_card_done_once() {
        let stub = Stub::new();
        let client = client(&stub.serve().await);
        let store = mem_store();
        let id = link(&store, "karta", "todo", "t-1");

        // A last_sync strictly after the card's updated_at ⇒ untouched card.
        let u0 = card(&store, &id).updated_at;
        let a1 = u0 + 100;
        sync_once(&store, &client, a1).await.unwrap();
        assert_eq!(meta(&store, "todoist_sync_at"), Some(a1.to_string()));

        let completed_at = a1 + 50;
        stub.complete("t-1", completed_at);
        let report = sync_once(&store, &client, a1 + 200).await.unwrap();
        assert_eq!(report.pulled, 1);
        let c = card(&store, &id);
        assert_eq!(c.status, "done");
        assert_eq!(c.done_at, Some(a1 + 200));
        assert_eq!(c.todoist_seen_at, completed_at);
        assert_eq!(c.todoist_state.as_deref(), Some("closed"));

        // Same completion again: nothing to pull or push.
        let before = stub.total();
        let report = sync_once(&store, &client, a1 + 260).await.unwrap();
        assert_eq!(report.pulled, 0);
        assert_eq!(stub.total(), before);
    }

    #[tokio::test]
    async fn manual_edit_wins_over_remote_completion() {
        let stub = Stub::new();
        let client = client(&stub.serve().await);
        let store = mem_store();
        let id = link(&store, "karta", "todo", "t-1");

        // last_sync before the card, so the later edit counts as manual.
        let u0 = card(&store, &id).updated_at;
        let a1 = u0 - 100;
        sync_once(&store, &client, a1).await.unwrap();

        move_card(&store, &id, "doing");
        let edited = card(&store, &id);
        assert!(edited.updated_at > a1);

        let completed_at = u0 - 50;
        stub.complete("t-1", completed_at);
        let report = sync_once(&store, &client, a1 + 200).await.unwrap();
        assert_eq!(report.reopened, 0);
        assert_eq!(stub.calls("reopen:t-1"), 1);
        let c = card(&store, &id);
        assert_eq!(c.status, "doing");
        assert_eq!(c.updated_at, edited.updated_at);
        assert_eq!(c.todoist_seen_at, completed_at);
        assert_eq!(c.todoist_state.as_deref(), Some("open"));

        // The settled event does not reopen twice.
        sync_once(&store, &client, a1 + 260).await.unwrap();
        assert_eq!(stub.calls("reopen:t-1"), 1);
    }

    #[tokio::test]
    async fn gone_card_unlinks_without_aborting_cycle() {
        let stub = Stub::new();
        let client = client(&stub.serve().await);
        let store = mem_store();
        let gone = link(&store, "znika", "done", "gone-1");
        let kept = link(&store, "zostaje", "done", "ok-1");
        stub.set_close("gone-1", StatusCode::NOT_FOUND);

        let report = sync_once(&store, &client, 1_000).await.unwrap();
        assert_eq!(stub.calls("close:ok-1"), 1);
        assert_eq!(card(&store, &kept).todoist_state.as_deref(), Some("closed"));
        assert!(card(&store, &gone).todoist_id.is_none());
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.unlinked, 1);
        assert!(report.errors[0].contains("gone-1"), "{:?}", report.errors);
    }

    #[tokio::test]
    async fn archived_card_is_pushed_but_never_pulled() {
        let stub = Stub::new();
        let client = client(&stub.serve().await);
        let store = mem_store();
        let id = link(&store, "archiwum", "done", "t-1");
        {
            let mut guard = store.lock().unwrap();
            let moved = guard
                .maintain(board::now() + board::RETENTION_SECS + 1)
                .unwrap();
            assert_eq!(moved, 1);
        }
        assert_eq!(card(&store, &id).status, "archived");
        stub.complete("t-1", 500);

        let report = sync_once(&store, &client, 1_000).await.unwrap();
        assert_eq!(report.closed, 1);
        assert_eq!(stub.calls("close:t-1"), 1);
        assert_eq!(stub.calls("completed"), 0);
        let c = card(&store, &id);
        assert_eq!(c.todoist_state.as_deref(), Some("closed"));
        assert_eq!(c.todoist_seen_at, 0);
    }

    #[tokio::test]
    async fn no_links_makes_no_requests() {
        let stub = Stub::new();
        let client = client(&stub.serve().await);
        let store = mem_store();

        let report = sync_once(&store, &client, 4_242).await.unwrap();
        assert_eq!(stub.total(), 0);
        assert!(report.errors.is_empty());
        assert_eq!(meta(&store, "todoist_sync_at"), Some("4242".to_string()));
    }

    #[tokio::test]
    async fn remote_errors_keep_last_sync() {
        let stub = Stub::new();
        let client = client(&stub.serve().await);
        let store = mem_store();
        let id = link(&store, "karta", "done", "t-1");
        stub.set_close("t-1", StatusCode::TOO_MANY_REQUESTS);

        let report = sync_once(&store, &client, 1_000).await.unwrap();
        assert!(!report.errors.is_empty());
        assert_eq!(meta(&store, "todoist_sync_at"), None);
        assert!(meta(&store, "todoist_last_error").is_some());
        assert_eq!(card(&store, &id).todoist_state.as_deref(), Some("open"));

        stub.set_close("t-1", StatusCode::INTERNAL_SERVER_ERROR);
        sync_once(&store, &client, 2_000).await.unwrap();
        assert_eq!(meta(&store, "todoist_sync_at"), None);
        assert!(meta(&store, "todoist_last_error").is_some());

        // Connection refused is a cycle error, not a panic.
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let dead = TodoistClient::new(&format!("http://{addr}"), TOKEN).unwrap();
        let report = sync_once(&store, &dead, 3_000).await.unwrap();
        assert!(!report.errors.is_empty());
        assert_eq!(meta(&store, "todoist_sync_at"), None);
    }

    #[tokio::test]
    async fn already_reopened_is_not_an_error() {
        let stub = Stub::new();
        let client = client(&stub.serve().await);
        let store = mem_store();
        let id = link(&store, "cykliczne", "done", "t-1");

        sync_once(&store, &client, 1_000).await.unwrap();
        assert_eq!(card(&store, &id).todoist_state.as_deref(), Some("closed"));

        move_card(&store, &id, "doing");
        stub.set_reopen("t-1", StatusCode::BAD_REQUEST);
        let report = sync_once(&store, &client, 2_000).await.unwrap();
        assert!(report.errors.is_empty());
        assert_eq!(stub.calls("reopen:t-1"), 1);
        assert_eq!(card(&store, &id).todoist_state.as_deref(), Some("open"));
    }

    #[tokio::test]
    async fn card_edited_during_pull_keeps_local_state() {
        let path = temp_db();
        let stub = Stub::new();
        let client = client(&stub.serve().await);
        let store = Mutex::new(Store::open(&path).unwrap());
        let id = link(&store, "karta", "todo", "t-1");

        let u0 = card(&store, &id).updated_at;
        let a1 = u0 + 100;
        sync_once(&store, &client, a1).await.unwrap();

        let completed_at = a1 + 50;
        stub.complete("t-1", completed_at);
        // A hand edit lands while the completion request is in flight.
        let bumped = u0 + 500;
        stub.touch_during_pull(&path, &id, bumped);

        let report = sync_once(&store, &client, a1 + 200).await.unwrap();
        assert_eq!(report.pulled, 1);
        let c = card(&store, &id);
        assert_eq!(c.status, "todo");
        assert_eq!(c.updated_at, bumped);
        assert_eq!(c.todoist_seen_at, completed_at);
        assert_eq!(c.todoist_state.as_deref(), Some("closed"));

        drop(store);
        cleanup(&path);
    }

    #[tokio::test]
    async fn completion_does_not_change_title_or_notes() {
        let stub = Stub::new();
        let client = client(&stub.serve().await);
        let store = mem_store();
        let id = link(&store, "Mleko", "todo", "t-1");

        let u0 = card(&store, &id).updated_at;
        let a1 = u0 + 100;
        sync_once(&store, &client, a1).await.unwrap();
        stub.complete("t-1", a1 + 50);
        sync_once(&store, &client, a1 + 200).await.unwrap();

        let c = card(&store, &id);
        assert_eq!(c.status, "done");
        assert_eq!(c.title, "Mleko");
        assert_eq!(c.notes, "");
    }

    #[tokio::test]
    async fn remote_reopen_does_not_revive_done_card() {
        let stub = Stub::new();
        let client = client(&stub.serve().await);
        let store = mem_store();
        let id = link(&store, "karta", "done", "t-1");

        // Cycle 1: card → DONE closes the task and records `closed`.
        sync_once(&store, &client, 1_000).await.unwrap();
        let closed = card(&store, &id);
        assert_eq!(closed.status, "done");
        assert_eq!(closed.todoist_state.as_deref(), Some("closed"));

        // Someone reopens the task in Todoist. The board is authoritative for
        // completion, so the next cycle must not touch the network at all.
        stub.active.lock().unwrap().insert("t-1".to_string());
        let before = stub.total();
        let report = sync_once(&store, &client, 2_000).await.unwrap();

        assert_eq!(stub.total(), before);
        assert_eq!(report.closed, 0);
        assert_eq!(report.reopened, 0);
        let after = card(&store, &id);
        assert_eq!(after.status, "done");
        assert_eq!(after.updated_at, closed.updated_at);
    }

    #[tokio::test]
    async fn only_changed_cards_are_closed() {
        let stub = Stub::new();
        let client = client(&stub.serve().await);
        let store = mem_store();
        let done = link(&store, "zrobione", "done", "t-1");
        let todo = link(&store, "do zrobienia", "todo", "t-2");
        let todo_before = card(&store, &todo);

        let report = sync_once(&store, &client, 1_000).await.unwrap();

        assert_eq!(report.closed, 1);
        assert_eq!(report.reopened, 0);
        assert_eq!(stub.calls("close:t-1"), 1);
        assert_eq!(stub.calls("close:t-2"), 0);
        assert_eq!(stub.calls("reopen:t-1"), 0);
        assert_eq!(stub.calls("reopen:t-2"), 0);
        assert_eq!(card(&store, &done).todoist_state.as_deref(), Some("closed"));
        let todo_after = card(&store, &todo);
        assert_eq!(todo_after.status, "todo");
        assert_eq!(todo_after.todoist_state.as_deref(), Some("open"));
        assert_eq!(todo_after.updated_at, todo_before.updated_at);
    }
}
