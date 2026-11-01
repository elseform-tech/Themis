//! Model-written tool arguments and person-written patterns are untrusted: reading which
//! sensitive files a call names must never panic, and neither may looking for a pasted key.
#![no_main]
use libfuzzer_sys::fuzz_target;
use serde_json::json;
use themis_core::secret_look::looks_like_secret;
use themis_core::tools::fuzzing::sensitive_names;

fuzz_target!(|data: (u8, Vec<String>, String, String)| {
    let (shape, patterns, command, text) = data;
    let tool = ["read_file", "shell", "git", "apply_patch", "write_file"][shape as usize % 5];
    let args = json!({
        "file_path": text, "patch": text, "command": command, "args": [text.clone(), command.clone()],
    });
    let _ = sensitive_names(&patterns, tool, &args);
    let _ = looks_like_secret(&text);
});
