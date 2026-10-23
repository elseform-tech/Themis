//! The editors Settings › Permissions can open `runtime.jsonc` with: the system text editor
//! plus the code editors installed here. Only these fixed ids are accepted, so no arbitrary
//! command ever runs.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// `(id, name, macOS app bundle, command on PATH)`.
const EDITORS: [(&str, &str, &str, &str); 8] = [
    ("vscode", "VS Code", "Visual Studio Code.app", "code"),
    ("cursor", "Cursor", "Cursor.app", "cursor"),
    ("windsurf", "Windsurf", "Windsurf.app", "windsurf"),
    (
        "antigravity",
        "Antigravity",
        "Antigravity.app",
        "antigravity",
    ),
    ("zed", "Zed", "Zed.app", "zed"),
    ("sublime", "Sublime Text", "Sublime Text.app", "subl"),
    ("bbedit", "BBEdit", "BBEdit.app", "bbedit"),
    ("xcode", "Xcode", "Xcode.app", "xed"),
];

/// Whether `id` names an editor this app knows (`""` is the system text editor).
pub fn is_known(id: &str) -> bool {
    id.is_empty() || EDITORS.iter().any(|(known, ..)| *known == id)
}

/// The system text editor (id `""`) and each installed code editor, as `{id, name}`.
pub fn installed_editors() -> Vec<Value> {
    let mut editors = vec![json!({"id": "", "name": "Text editor"})];
    editors.extend(
        EDITORS
            .iter()
            .filter(|(_, _, app, command)| locate(app, command).is_some())
            .map(|(id, name, _, _)| json!({"id": id, "name": name})),
    );
    editors
}

/// Opens `target` with editor `id` (`""` is the system text editor).
pub fn open_with(id: &str, target: &Path) -> Result<(), String> {
    if id.is_empty() {
        return super::spawn_text_editor(target);
    }
    let (_, name, app, command) = EDITORS
        .iter()
        .find(|(known, ..)| *known == id)
        .ok_or_else(|| format!("unknown editor '{id}'"))?;
    let program = locate(app, command).ok_or_else(|| format!("{name} is not installed"))?;
    let mut launch = if cfg!(target_os = "macos") {
        let mut open = std::process::Command::new("open");
        open.arg("-a").arg(program);
        open
    } else {
        std::process::Command::new(program)
    };
    launch
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("failed to open '{}' with {name}: {err}", target.display()))
}

/// Where an editor is installed: its app bundle on macOS, otherwise its command on `PATH`.
fn locate(app: &str, command: &str) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        return [
            Some(PathBuf::from("/Applications")),
            home.map(|h| h.join("Applications")),
        ]
        .into_iter()
        .flatten()
        .map(|dir| dir.join(app))
        .find(|bundle| bundle.is_dir());
    }
    let names: &[String] = &if cfg!(windows) {
        vec![format!("{command}.cmd"), format!("{command}.exe")]
    } else {
        vec![command.to_owned()]
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_editor_is_always_offered_first() {
        let editors = installed_editors();
        assert_eq!(editors[0], json!({"id": "", "name": "Text editor"}));
        assert!(editors[1..]
            .iter()
            .all(|e| EDITORS.iter().any(|(id, ..)| e["id"] == *id)));
    }

    #[test]
    fn an_unknown_editor_runs_nothing() {
        let error = open_with("rm -rf", Path::new("/tmp/x")).unwrap_err();
        assert!(error.contains("unknown editor"), "{error}");
    }
}
