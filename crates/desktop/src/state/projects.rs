use super::*;

impl AppState {
    /// The managed project is rooted beside app data, independent of the
    /// configurable folder used for new user projects.
    pub async fn get_default_project(&self) -> Result<ProjectInfo, String> {
        let _initialization = self.inner.default_project_init.lock().await;
        let directory = settings_app_dir(self.inner.settings.path());
        let root = directory.join("Themis");
        if !root.exists() {
            self.create_project(
                "Themis".to_owned(),
                Some(directory.to_string_lossy().into_owned()),
            )
            .await?;
        }
        self.open_project(root.to_string_lossy().into_owned()).await
    }

    /// Creates a new project without touching any existing directory.
    pub async fn create_project(
        &self,
        name: String,
        directory: Option<String>,
    ) -> Result<ProjectInfo, String> {
        let name = name.trim();
        if name.is_empty()
            || name.len() > 80
            || name.starts_with('.')
            || name.ends_with('.')
            || !name
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
        {
            return Err(
                "Use 1–80 letters, numbers, spaces, hyphens or underscores for the project name"
                    .to_owned(),
            );
        }
        let base = directory.unwrap_or(self.inner.settings.get().await.projects_directory);
        let base = PathBuf::from(base.trim());
        if !base.is_absolute()
            || base
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err("Choose an absolute projects folder in Settings".to_owned());
        }
        std::fs::create_dir_all(&base)
            .map_err(|err| format!("Could not create projects folder: {err}"))?;
        let root = base
            .canonicalize()
            .map_err(|err| err.to_string())?
            .join(name);
        std::fs::create_dir(&root).map_err(|err| {
            if err.kind() == std::io::ErrorKind::AlreadyExists {
                "A project with this name already exists. Choose another name or open the existing folder.".to_owned()
            } else {
                format!("Could not create project: {err}")
            }
        })?;
        // A first commit establishes the project diff baseline.
        let initialized = (|| {
            for args in [
                vec!["init", "-q", "-b", "main"],
                vec![
                    "-c",
                    "user.name=Themis",
                    "-c",
                    "user.email=themis@localhost",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                    "commit",
                    "--allow-empty",
                    "-qm",
                    "Initialize project",
                ],
            ] {
                let output = std::process::Command::new("git")
                    .env_remove("OPENCODE_KEY")
                    .current_dir(&root)
                    .args(args)
                    .output()
                    .map_err(|err| err.to_string())?;
                if !output.status.success() {
                    return Err(String::from_utf8_lossy(&output.stderr).into_owned());
                }
            }
            Ok::<(), String>(())
        })();
        if let Err(error) = initialized {
            return Err(format!(
                "Project folder created at {}, but Git setup failed: {error}",
                root.display()
            ));
        }
        self.open_project(root.to_string_lossy().into_owned()).await
    }

    /// Opens a project directory by typed path and reports its git status.
    pub async fn open_project(&self, path: String) -> Result<ProjectInfo, String> {
        let root = canonical_project_dir(&path)?;
        let name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let info = ProjectInfo {
            root: root.to_string_lossy().into_owned(),
            name,
            is_git: is_git_repo(&root),
            is_default: settings_app_dir(self.inner.settings.path())
                .join("Themis")
                .canonicalize()
                .is_ok_and(|default| default == root),
        };
        self.inner
            .projects
            .write()
            .await
            .insert(info.root.clone(), info.clone());
        // Track recency for the project picker (best-effort).
        let mut recent = self.inner.settings.get().await.recent_roots;
        recent.retain(|root| root != &info.root);
        recent.insert(0, info.root.clone());
        recent.truncate(crate::settings::MAX_RECENT_ROOTS);
        let _ = self
            .inner
            .settings
            .update(SettingsPatch {
                recent_roots: Some(recent),
                ..SettingsPatch::default()
            })
            .await;
        Ok(info)
    }
}
