//! Putting a plugin's files in place in a project, and its own data folder.

use super::*;

impl PluginStore {
    /// `CLAUDE_PLUGIN_DATA`: a plugin's own folder, in app data and kept until it is uninstalled.
    #[must_use]
    pub fn plugin_data_dir(&self, scope: &str, name: &str) -> PathBuf {
        self.global
            .join("plugin-data")
            .join(format!("{scope}--{name}"))
    }

    pub fn materialize(&self, plugins: &[Plugin], root: &Path) -> anyhow::Result<()> {
        for plugin in plugins {
            let directory = root
                .join(".themis/plugin-files")
                .join(plugin.skill_id("resources"));
            // Two runs of one project may start together: one must not remove the other's work.
            let _materializing = MATERIALIZING.lock().unwrap_or_else(|e| e.into_inner());
            // A project's copy can be written by a repository or anything outside the sandbox,
            // so each run checks it all and puts back what was installed.
            let prepare = |parent: &Path| -> anyhow::Result<()> {
                safe_create_parent(root, parent)?;
                crate::skills::ensure_git_excluded(&root.join(".themis/plugin-files"))?;
                Ok(())
            };
            let mut wanted: std::collections::BTreeSet<String> =
                plugin.spec.files.keys().cloned().collect();
            if let Some(kept) = &plugin.spec.package {
                let store = self.directory(&plugin.scope)?.join(package::PACKAGES_DIR);
                match package::verified(&store, kept)? {
                    Some(entries) => {
                        package::copy_to(&entries, &directory, &prepare)?;
                        wanted.extend(package::names(&entries));
                    }
                    // Its components still work, and a hook that needs the rest fails as it
                    // would for any missing script, so a guard still blocks.
                    None => crate::diagnostics::emit(
                        "plugins",
                        "package_missing",
                        "warn",
                        serde_json::json!({"plugin": plugin.spec.name}),
                    ),
                }
            }
            package::remove_others(&directory, &wanted)?;
            for (relative, content) in &plugin.spec.files {
                let path = directory.join(relative);
                let parent = path.parent().context("Missing parent")?;
                safe_create_parent(root, parent)?;
                crate::skills::ensure_git_excluded(&root.join(".themis/plugin-files"))?;
                if path
                    .symlink_metadata()
                    .is_ok_and(|m| m.file_type().is_symlink())
                {
                    bail!("Symlink resource target rejected");
                }
                // Persistent MCP processes may be reading this revision concurrently.
                // Not through a link: `fs::read` would follow one and compare what it points at.
                if !path
                    .symlink_metadata()
                    .is_ok_and(|metadata| metadata.is_file())
                    || !fs::read(&path).is_ok_and(|bytes| bytes == content.as_bytes())
                {
                    write_bytes(
                        &path,
                        content.as_bytes(),
                        plugin.spec.executable_files.contains(relative),
                    )?;
                }
                #[cfg(unix)]
                if plugin.spec.executable_files.contains(relative) {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
                }
            }
        }
        Ok(())
    }
}

/// Held while a plugin's files are put in place in a project.
static MATERIALIZING: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(super) fn safe_create_parent(root: &Path, parent: &Path) -> anyhow::Result<()> {
    let canonical = root.canonicalize()?;
    let relative = parent.strip_prefix(root)?;
    let mut next = root.to_path_buf();
    for part in relative.components() {
        next.push(part);
        if next.exists() {
            if !next.canonicalize()?.starts_with(&canonical) {
                bail!("Resource path escapes project");
            }
        } else {
            fs::create_dir(&next)?;
        }
    }
    Ok(())
}
