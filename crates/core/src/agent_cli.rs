//! The `themis` CLI in an agent's shell.
//!
//! Each run's commands get a short-lived credential naming the run and its chat, the port of the
//! live app server and a folder holding only the app's own `themis` at the front of `PATH`. The
//! app server issues the credential (see `crates/desktop/src/server/agent.rs`), treats every call
//! that carries it as the agent's and refuses the methods an agent may not use. The person's own
//! credential stays in the server's discovery file, which sandboxed commands cannot read (see
//! [`DISCOVERY_FILE`] and `docs/sandbox.md`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// The credential of the run, in the environment of its commands.
pub const TOKEN_ENV: &str = "THEMIS_AGENT_TOKEN";
/// The local port of the app server, in the environment of a run's commands.
pub const PORT_ENV: &str = "THEMIS_AGENT_PORT";
/// The app server's discovery file in the app-data folder. It holds the person's credential, so
/// sandboxed commands can neither read nor write it.
pub const DISCOVERY_FILE: &str = "server.json";

/// What one run's commands get: its credential, the server's port and the folder that holds the
/// app's `themis` (none when the app ships no console CLI here).
#[derive(Clone, PartialEq, Eq)]
pub struct Access {
    token: String,
    port: u16,
    bin_dir: Option<PathBuf>,
}

impl std::fmt::Debug for Access {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the credential.
        formatter
            .debug_struct("Access")
            .field("port", &self.port)
            .field("bin_dir", &self.bin_dir)
            .finish_non_exhaustive()
    }
}

impl Access {
    pub fn new(token: String, port: u16, bin_dir: Option<PathBuf>) -> Self {
        Self {
            token,
            port,
            bin_dir,
        }
    }

    /// The variables a command of the run gets.
    pub fn env(&self) -> [(&'static str, String); 2] {
        [
            (TOKEN_ENV, self.token.clone()),
            (PORT_ENV, self.port.to_string()),
        ]
    }

    /// The folder put first on the command's `PATH`.
    pub fn bin_dir(&self) -> Option<&Path> {
        self.bin_dir.as_deref()
    }

    /// `text` with the credential masked, for output that goes back to the model.
    pub fn redact(&self, text: &str) -> String {
        if self.token.is_empty() || !text.contains(&self.token) {
            return text.to_owned();
        }
        text.replace(&self.token, "[redacted]")
    }
}

/// A tool result with every string in it masked by `access` (unchanged without one).
pub fn redact_value(value: serde_json::Value, access: Option<&Access>) -> serde_json::Value {
    use serde_json::Value;
    let Some(access) = access else {
        return value;
    };
    match value {
        Value::String(text) => Value::String(access.redact(&text)),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| redact_value(item, Some(access)))
                .collect(),
        ),
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .map(|(key, item)| (key, redact_value(item, Some(access))))
                .collect(),
        ),
        other => other,
    }
}

/// Issues and revokes run credentials (the app server).
pub trait Issuer: Send + Sync {
    /// The access for one run of a chat, issued once per run and reused by its later commands.
    fn issue(&self, thread_id: &str, run_id: &str) -> Option<Access>;
    /// Ends every credential the chat's runs hold.
    fn revoke_thread(&self, thread_id: &str);
}

/// The issuer of this process. One app server per process installs it.
static ISSUER: RwLock<Option<Arc<dyn Issuer>>> = RwLock::new(None);

/// Makes `issuer` the one commands of runs ask.
pub fn install(issuer: Arc<dyn Issuer>) {
    *ISSUER.write().unwrap_or_else(|error| error.into_inner()) = Some(issuer);
}

/// Removes `issuer` if it is still the installed one (a server that stops).
pub fn uninstall(issuer: &Arc<dyn Issuer>) {
    let mut slot = ISSUER.write().unwrap_or_else(|error| error.into_inner());
    if slot
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, issuer))
    {
        *slot = None;
    }
}

fn installed() -> Option<Arc<dyn Issuer>> {
    ISSUER
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

/// The access for a run of `thread_id`, when an app server runs in this process.
pub fn issue(thread_id: &str, run_id: &str) -> Option<Access> {
    if thread_id.is_empty() || run_id.is_empty() {
        return None;
    }
    installed()?.issue(thread_id, run_id)
}

/// The access for the run whose task this is (from its diagnostics context), if any.
pub fn current() -> Option<Access> {
    let (thread_id, run_id) = crate::diagnostics::ACTIVE
        .try_with(|context| (context.thread_id.clone(), context.run_id.clone()))
        .ok()?;
    issue(&thread_id, &run_id)
}

/// Ends the credentials of a chat's runs (its run finished).
pub fn revoke_thread(thread_id: &str) {
    if let Some(issuer) = installed() {
        issuer.revoke_thread(thread_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_credential_never_shows_in_debug_output_and_is_masked_in_text() {
        let access = Access::new("secret-run-token".into(), 4711, None);
        assert!(!format!("{access:?}").contains("secret-run-token"));
        assert_eq!(
            access.redact("token=secret-run-token done"),
            "token=[redacted] done"
        );
        assert_eq!(access.redact("nothing here"), "nothing here");
        let value = serde_json::json!({"text": "a secret-run-token b", "n": 1,
            "list": ["secret-run-token"]});
        assert_eq!(
            redact_value(value.clone(), Some(&access)),
            serde_json::json!({"text": "a [redacted] b", "n": 1, "list": ["[redacted]"]})
        );
        assert_eq!(redact_value(value.clone(), None), value);
        let env = access.env();
        assert_eq!(env[0], (TOKEN_ENV, "secret-run-token".to_owned()));
        assert_eq!(env[1], (PORT_ENV, "4711".to_owned()));
    }

    #[test]
    fn nothing_is_issued_outside_a_run() {
        assert!(current().is_none());
        assert!(issue("", "run").is_none());
        assert!(issue("thread", "").is_none());
    }
}
