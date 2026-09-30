//! Non-destructive diagnostics. Never constructs AppState or initializes stores.
use std::collections::HashSet;
use std::path::Path;

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::server::Client;
use crate::state::RegistryEntry;
use crate::transcript::TranscriptStore;
use crate::types::{Automation, ReviewItem, Settings, Skill};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Warn,
    Fail,
    Unverified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub status: Status,
    pub detail: String,
    pub action: String,
}

impl Check {
    pub(crate) fn new(name: &str, status: Status, detail: impl Into<String>, action: &str) -> Self {
        Self {
            name: name.to_owned(),
            status,
            detail: detail.into(),
            action: action.to_owned(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub version: &'static str,
    pub data_dir: String,
    pub checks: Vec<Check>,
}

impl Report {
    pub fn failed(&self) -> bool {
        self.checks.iter().any(|check| check.status == Status::Fail)
    }
}

fn json_store<T: DeserializeOwned>(
    directory: &Path,
    name: &str,
    checks: &mut Vec<Check>,
) -> Option<T> {
    match std::fs::read(directory.join(name)) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(value) => {
                checks.push(Check::new(
                    name,
                    Status::Pass,
                    "Readable and valid for the current schema",
                    "",
                ));
                Some(value)
            }
            Err(error) => {
                // Serde error messages can contain store contents. Report location only.
                checks.push(Check::new(
                    name,
                    Status::Fail,
                    format!(
                        "Invalid store at line {}, column {}",
                        error.line(),
                        error.column()
                    ),
                    "Back up the data directory and recover this file; no repair was attempted",
                ));
                None
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            checks.push(Check::new(
                name,
                Status::Warn,
                "Missing; defaults or an empty store will be used",
                "Expected before first use; recover from backup if data was previously present",
            ));
            None
        }
        Err(_) => {
            checks.push(Check::new(
                name,
                Status::Fail,
                "Store cannot be read",
                "Check file ownership and read permissions",
            ));
            None
        }
    }
}

pub(crate) fn store_checks(directory: &Path) -> (Vec<Check>, Option<HashSet<String>>) {
    let mut checks = Vec::new();
    json_store::<Settings>(directory, "settings.json", &mut checks);
    let threads = json_store::<Vec<RegistryEntry>>(directory, "threads.json", &mut checks);
    let skills = json_store::<Vec<Skill>>(directory, "skills.json", &mut checks);
    let automations = json_store::<Vec<Automation>>(directory, "automations.json", &mut checks);
    let reviews = json_store::<Vec<ReviewItem>>(directory, "review_items.json", &mut checks);
    let thread_ids = threads.as_ref().map(|threads| {
        threads
            .iter()
            .map(|thread| thread.id.clone())
            .collect::<HashSet<_>>()
    });
    let skill_ids = skills.as_ref().map(|skills| {
        skills
            .iter()
            .map(|skill| skill.id.as_str())
            .collect::<HashSet<_>>()
    });
    let duplicate = threads.as_ref().is_some_and(|entries| {
        thread_ids
            .as_ref()
            .is_some_and(|ids| ids.len() != entries.len())
    }) || skills.as_ref().is_some_and(|entries| {
        skill_ids
            .as_ref()
            .is_some_and(|ids| ids.len() != entries.len())
    }) || automations.as_ref().is_some_and(|entries| {
        entries
            .iter()
            .map(|entry| &entry.id)
            .collect::<HashSet<_>>()
            .len()
            != entries.len()
    }) || reviews.as_ref().is_some_and(|entries| {
        entries
            .iter()
            .map(|entry| &entry.id)
            .collect::<HashSet<_>>()
            .len()
            != entries.len()
    });
    if duplicate {
        checks.push(Check::new(
            "store_ids",
            Status::Fail,
            "Duplicate identifiers would overwrite entries on load",
            "Back up the stores and resolve duplicate identifiers",
        ));
    }
    if let (Some(threads), Some(skills)) = (&threads, &skill_ids) {
        let missing = threads
            .iter()
            .flat_map(|thread| &thread.skill_ids)
            .filter(|id| !skills.contains(id.as_str()))
            .count();
        if missing != 0 {
            checks.push(Check::new(
                "skill_references",
                Status::Warn,
                format!("{missing} thread skill references are missing"),
                "Review attached skills; unavailable skills are skipped",
            ));
        }
    }
    (checks, thread_ids)
}

pub async fn diagnose(directory: &Path, deep: bool) -> Report {
    let (mut checks, thread_ids) = store_checks(directory);
    checks.insert(
        0,
        Check::new(
            "installation",
            Status::Pass,
            std::env::current_exe().map_or_else(
                |_| "Executable path unavailable".to_owned(),
                |path| path.display().to_string(),
            ),
            "",
        ),
    );
    let client = Client::new(directory.to_path_buf());
    let live = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.call("get_diagnostics", serde_json::json!({"deep": deep})),
    )
    .await;
    let mut lock_busy = false;
    if let Ok(lock) = std::fs::File::open(directory.join("server.lock")) {
        lock_busy = lock.try_lock().is_err();
    }
    #[cfg(unix)]
    if let Ok(metadata) = std::fs::metadata(directory.join("server.json")) {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            checks.push(Check::new(
                "discovery_permissions",
                Status::Fail,
                "Server discovery credentials are accessible to other users",
                "Restrict server.json to owner read/write (0600)",
            ));
        } else {
            checks.push(Check::new(
                "discovery_permissions",
                Status::Pass,
                "Server discovery is restricted to its owner",
                "",
            ));
        }
    }
    match live {
        Ok(Ok(diagnostics)) => {
            let matching = diagnostics["app_version"] == env!("CARGO_PKG_VERSION");
            checks.push(Check::new(
                "server",
                if matching { Status::Pass } else { Status::Warn },
                "Authenticated local server answered",
                if matching {
                    ""
                } else {
                    "Restart the server to match the installed CLI version"
                },
            ));
            if let Ok(persistence) =
                serde_json::from_value::<Vec<Check>>(diagnostics["persistence"].clone())
            {
                checks.extend(persistence);
            } else {
                checks.push(Check::new(
                    "sessions",
                    Status::Unverified,
                    "Server does not expose persistence diagnostics",
                    "Update and restart the server",
                ));
            }
            let errors = diagnostics["recent_errors"].as_array().map_or(0, Vec::len);
            checks.push(Check::new(
                "recent_errors",
                if errors == 0 {
                    Status::Pass
                } else {
                    Status::Warn
                },
                format!("{errors} recent backend errors; error contents omitted"),
                if errors == 0 {
                    ""
                } else {
                    "Review diagnostics locally in Settings"
                },
            ));
            let recovery = diagnostics["recovery_notes"].as_u64().unwrap_or(0);
            checks.push(Check::new(
                "startup_recovery",
                if recovery == 0 {
                    Status::Pass
                } else {
                    Status::Warn
                },
                format!("{recovery} startup recovery notes; interrupted runs return to idle"),
                "Review recovered conversations before continuing",
            ));
            checks.push(Check::new(
                "scheduler",
                Status::Unverified,
                "Server is running; scheduling success has not been exercised",
                "Check automation results in the review queue",
            ));
            let secrets = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                client.call("get_secret_status", serde_json::Value::Null),
            )
            .await;
            checks.push(Check::new(
                "provider_key",
                if secrets.is_ok_and(|result| result.is_ok_and(|status| status["go"] == true)) {
                    Status::Pass
                } else {
                    Status::Warn
                },
                "Key availability checked without an authentication request",
                "Use themis providers or Settings to configure OpenCode Go",
            ));
        }
        _ => {
            checks.push(Check::new(
                "server",
                Status::Warn,
                if lock_busy {
                    "Server lock is held but the authenticated server did not answer"
                } else {
                    "Server is offline or discovery is stale; startup was not attempted"
                },
                "Run themis server start after resolving persistence failures",
            ));
            if lock_busy {
                checks.push(Check::new(
                    "sessions",
                    Status::Unverified,
                    "Live owner unavailable; database was not opened",
                    "Restore server connectivity and rerun doctor",
                ));
            } else {
                checks.extend(TranscriptStore::diagnose_read_only(
                    &directory.join("sessions.sqlite3"),
                    deep,
                    thread_ids.as_ref(),
                ));
            }
            checks.push(Check::new(
                "provider_key",
                Status::Unverified,
                "Offline; credential store and provider network were not accessed",
                "Start the server and run themis auth status",
            ));
        }
    }
    checks.push(Check::new("durability", Status::Unverified, "Read checks do not prove successful writes or crash durability; no write probe or repair was performed", "Use isolated persistence/restart tests before relying on durability"));
    Report {
        version: env!("CARGO_PKG_VERSION"),
        data_dir: directory.display().to_string(),
        checks,
    }
}
