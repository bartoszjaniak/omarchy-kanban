mod board;
mod bridge;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(version, about = "Personal Kanban board: TODO / In Progress / DONE + archive")]
struct Cli {
    /// Data directory holding the board database and token.
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the local board service (SQLite + HTTP API).
    Serve {
        #[arg(long, default_value_t = 4765)]
        port: u16,
    },
    /// JSON-lines client for the Omarchy widget.
    Bridge,
    /// Read and change tasks; used by the AI via the shell.
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
}

#[derive(Subcommand)]
enum TaskCommand {
    /// List tasks grouped by column.
    List {
        /// Include the hidden archive.
        #[arg(long)]
        all: bool,
        /// Machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Add a task.
    Add {
        title: String,
        /// todo (default), doing or done.
        #[arg(long, default_value = "todo")]
        status: String,
        #[arg(long)]
        notes: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Move a task to another column.
    Move {
        id: String,
        /// todo, doing or done.
        status: String,
    },
    /// Change a task's title or notes.
    Edit {
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        notes: Option<String>,
    },
    /// Delete a task.
    Rm { id: String },
}

pub fn user_dir(kind: &str) -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let env = match kind {
        "config" => "XDG_CONFIG_HOME",
        "cache" => "XDG_CACHE_HOME",
        "state" => "XDG_STATE_HOME",
        _ => "XDG_DATA_HOME",
    };
    let fallback = if cfg!(target_os = "macos") && kind == "data" {
        home.join("Library/Application Support")
    } else {
        home.join(match kind {
            "config" => ".config",
            "cache" => ".cache",
            "state" => ".local/state",
            _ => ".local/share",
        })
    };
    std::env::var_os(env)
        .map(PathBuf::from)
        .unwrap_or(fallback)
        .join("agent-kanban")
}

pub fn private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        private_dir(parent)?;
    }
    let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(temp, path)?;
    Ok(())
}

struct Api {
    endpoint: String,
    token: String,
    client: reqwest::Client,
}

impl Api {
    fn connect(data_dir: &Path) -> Result<Self> {
        let identity: Value = serde_json::from_slice(
            &std::fs::read(data_dir.join("identity.json")).context(
                "Core nie działa. Uruchom: systemctl --user start agent-kanban.service",
            )?,
        )?;
        let port = identity["port"]
            .as_u64()
            .context("Brak portu w identity.json")?;
        let token = std::fs::read_to_string(data_dir.join("token"))
            .context("Brak tokenu Core")?
            .trim()
            .to_string();
        Ok(Self {
            endpoint: format!("http://127.0.0.1:{port}"),
            token,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(6))
                .no_proxy()
                .build()?,
        })
    }

    async fn call(&self, method: reqwest::Method, path: &str, body: Value) -> Result<Value> {
        let mut request = self
            .client
            .request(method, format!("{}{}", self.endpoint, path))
            .bearer_auth(&self.token);
        if !body.is_null() {
            request = request.json(&body);
        }
        let response = request.send().await.context("Brak połączenia z Core")?;
        let status = response.status();
        let value: Value = response.json().await.unwrap_or(json!({}));
        if !status.is_success() {
            bail!(
                "{}",
                value["error"].as_str().unwrap_or("Błąd żądania do Core")
            );
        }
        Ok(value)
    }
}

fn short(id: &str) -> &str {
    &id[..id.len().min(8)]
}

fn parse_tasks(state: &Value) -> Result<Vec<board::Task>> {
    Ok(serde_json::from_value(
        state["tasks"].clone(),
    )?)
}

fn print_list(tasks: &[board::Task], all: bool) {
    for (status, label) in [
        ("todo", "TODO"),
        ("doing", "IN PROGRESS"),
        ("done", "DONE"),
    ] {
        let column: Vec<_> = tasks.iter().filter(|t| t.status == status).collect();
        println!("{label} ({})", column.len());
        for task in column {
            println!("  {}  {}", short(&task.id), task.title);
            if !task.notes.is_empty() {
                println!("            {}", task.notes.replace('\n', " "));
            }
        }
    }
    let archived = tasks.iter().filter(|t| t.status == "archived").count();
    if all {
        println!("ARCHIWUM ({archived})");
        for task in tasks.iter().filter(|t| t.status == "archived") {
            println!("  {}  {}", short(&task.id), task.title);
        }
    } else {
        println!("Archiwum: {archived} (ukryte; --all aby pokazać)");
    }
}

async fn task_command(data_dir: &Path, command: TaskCommand) -> Result<()> {
    let api = Api::connect(data_dir)?;
    match command {
        TaskCommand::List { all, json } => {
            let state = api.call(reqwest::Method::GET, "/api/state", Value::Null).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&state)?);
                return Ok(());
            }
            let tasks = parse_tasks(&state)?;
            print_list(&tasks, all);
        }
        TaskCommand::Add {
            title,
            status,
            notes,
            json,
        } => {
            let body = json!({"title": title, "status": status, "notes": notes});
            let result = api.call(reqwest::Method::POST, "/api/tasks", body).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
                return Ok(());
            }
            let task: board::Task = serde_json::from_value(result["task"].clone())?;
            println!("Dodano {} [{}] {}", short(&task.id), task.status, task.title);
        }
        TaskCommand::Move { id, status } => {
            let result = api
                .call(
                    reqwest::Method::PATCH,
                    &format!("/api/tasks/{id}"),
                    json!({"status": status}),
                )
                .await?;
            let task: board::Task = serde_json::from_value(result["task"].clone())?;
            println!("{} → [{}] {}", short(&task.id), task.status, task.title);
        }
        TaskCommand::Edit { id, title, notes } => {
            let body = json!({"title": title, "notes": notes});
            let result = api
                .call(reqwest::Method::PATCH, &format!("/api/tasks/{id}"), body)
                .await?;
            let task: board::Task = serde_json::from_value(result["task"].clone())?;
            println!("Zaktualizowano {} {}", short(&task.id), task.title);
        }
        TaskCommand::Rm { id } => {
            let result = api
                .call(
                    reqwest::Method::DELETE,
                    &format!("/api/tasks/{id}"),
                    Value::Null,
                )
                .await?;
            let task: board::Task = serde_json::from_value(result["task"].clone())?;
            println!("Usunięto {} {}", short(&task.id), task.title);
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let data_dir = cli
        .data_dir
        .clone()
        .unwrap_or_else(|| user_dir("data"));
    match cli.command.unwrap_or(Command::Serve { port: 4765 }) {
        Command::Serve { port } => board::serve(data_dir, port).await,
        Command::Bridge => bridge::run(data_dir).await,
        Command::Task { command } => task_command(&data_dir, command).await,
    }
}
