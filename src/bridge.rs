//! JSON-lines client used by the Omarchy widget.
//!
//! Reads board state from the local Core and forwards widget actions to it.
//! Prints one JSON object per line on stdout; reads one JSON action per line
//! on stdin.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

async fn emit(value: Value) -> Result<()> {
    let mut out = tokio::io::stdout();
    let mut bytes = serde_json::to_vec(&value)?;
    bytes.push(b'\n');
    out.write_all(&bytes).await?;
    out.flush().await?;
    Ok(())
}

async fn response(response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    let value = response.json::<Value>().await?;
    if !status.is_success() {
        bail!(
            "{}",
            value["error"].as_str().unwrap_or("Błąd żądania do Core")
        );
    }
    Ok(value)
}

struct Core {
    port: u16,
    token: String,
    client: reqwest::Client,
}

impl Core {
    fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
    async fn state(&self) -> Result<Value> {
        response(
            self.client
                .get(format!("{}/api/state", self.endpoint()))
                .timeout(Duration::from_secs(4))
                .bearer_auth(&self.token)
                .send()
                .await?,
        )
        .await
    }
    async fn task(&self, method: reqwest::Method, path: String, body: Value) -> Result<Value> {
        let mut request = self
            .client
            .request(method, format!("{}{}", self.endpoint(), path))
            .bearer_auth(&self.token);
        if !body.is_null() {
            request = request.json(&body);
        }
        response(request.send().await?).await
    }
}

fn load_core(data_dir: &std::path::Path) -> Result<Core> {
    let identity: Value = serde_json::from_slice(
        &std::fs::read(data_dir.join("identity.json"))
            .context("Core nie uruchomił się jeszcze (brak identity.json)")?,
    )?;
    let port = identity["port"]
        .as_u64()
        .context("Brak portu w identity.json")? as u16;
    let token = std::fs::read_to_string(data_dir.join("token"))
        .context("Brak tokenu Core")?
        .trim()
        .to_string();
    Ok(Core {
        port,
        token,
        client: reqwest::Client::builder()
            .timeout(Duration::from_secs(6))
            .no_proxy()
            .build()?,
    })
}

async fn apply(core: &Core, action: Value) -> Result<()> {
    match action["action"].as_str().unwrap_or("") {
        "refresh" => {}
        "add" => {
            let title = action["title"].as_str().context("Brak tytułu zadania")?;
            core.task(
                reqwest::Method::POST,
                "/api/tasks".into(),
                json!({"title": title, "notes": action["notes"], "status": action["status"]}),
            )
            .await?;
        }
        "move" => {
            let id = action["id"].as_str().context("Brak id zadania")?;
            let status = action["status"].as_str().context("Brak statusu")?;
            let mut body = json!({"status": status});
            if let Some(position) = action["position"].as_f64() {
                body["position"] = json!(position);
            }
            core.task(
                reqwest::Method::PATCH,
                format!("/api/tasks/{id}"),
                body,
            )
            .await?;
        }
        "edit" => {
            let id = action["id"].as_str().context("Brak id zadania")?;
            let mut body = json!({});
            for key in ["title", "notes", "status"] {
                if !action[key].is_null() {
                    body[key] = action[key].clone();
                }
            }
            core.task(reqwest::Method::PATCH, format!("/api/tasks/{id}"), body)
                .await?;
        }
        "delete" => {
            let id = action["id"].as_str().context("Brak id zadania")?;
            core.task(
                reqwest::Method::DELETE,
                format!("/api/tasks/{id}"),
                Value::Null,
            )
            .await?;
        }
        other => bail!("Nieznane działanie: {other}"),
    }
    Ok(())
}

fn payload(state: Option<&Value>, error: Option<String>) -> Value {
    let empty = json!({"tasks": [], "counts": {}});
    let state = state.unwrap_or(&empty);
    json!({
        "tasks": state["tasks"].clone(),
        "counts": state["counts"].clone(),
        "core_id": state["core_id"].clone(),
        "connected": error.is_none(),
        "error": error,
    })
}

pub async fn run(data_dir: PathBuf) -> Result<()> {
    let core = load_core(&data_dir)?;
    let mut cache: Option<Value> = None;
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut input = BufReader::new(tokio::io::stdin()).lines();
    loop {
        let line = tokio::select! {
            _ = interval.tick() => None,
            line = input.next_line() => line?,
        };
        let mut error = None;
        if let Some(line) = line {
            let action: Value =
                serde_json::from_str(&line).unwrap_or(json!({"action": "invalid"}));
            if let Err(e) = apply(&core, action).await {
                error = Some(e.to_string());
            }
        }
        match core.state().await {
            Ok(state) => {
                cache = Some(state);
            }
            Err(e) => {
                if error.is_none() {
                    error = Some(format!("Brak połączenia z Core: {e}"));
                }
            }
        }
        emit(payload(cache.as_ref(), error)).await?;
    }
}
