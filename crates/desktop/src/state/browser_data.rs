//! Settings › Browser: the built-in browser's own history per project, removing entries, and
//! clearing browsing data by time range and type. Chrome keeps these in its profile's SQLite
//! files and locks them while it runs, so reads use a copy and changes close that project's
//! browser first; a run using the browser refuses changes.

use super::*;
use serde_json::{json, Value};

/// Microseconds between 1601-01-01 (Chrome's epoch) and 1970-01-01.
const CHROME_EPOCH_OFFSET_US: i64 = 11_644_473_600_000_000;
/// History rows one call returns at most.
const HISTORY_LIMIT: usize = 500;

fn chrome_time_from_unix(seconds: i64) -> i64 {
    seconds * 1_000_000 + CHROME_EPOCH_OFFSET_US
}

fn unix_from_chrome_time(chrome: i64) -> i64 {
    (chrome - CHROME_EPOCH_OFFSET_US) / 1_000_000
}

/// Where Chrome keeps one kind of data in a profile (newer versions moved cookies).
fn cookies_db(profile: &Path) -> PathBuf {
    let network = profile.join("Default").join("Network").join("Cookies");
    if network.exists() {
        network
    } else {
        profile.join("Default").join("Cookies")
    }
}

/// Reads history rows from a copy of the profile's `History` database.
fn read_history(profile: &Path, query: &str, limit: usize) -> Result<Vec<Value>, String> {
    let source = profile.join("Default").join("History");
    if !source.exists() {
        return Ok(Vec::new());
    }
    // Chrome locks the file while it runs: read a copy, removed when done.
    struct Copy(PathBuf);
    impl Drop for Copy {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let copy =
        Copy(std::env::temp_dir().join(format!("themis-history-{}.db", uuid::Uuid::new_v4())));
    std::fs::copy(&source, &copy.0)
        .map_err(|e| format!("could not read the browser history: {e}"))?;
    let connection =
        rusqlite::Connection::open_with_flags(&copy.0, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| format!("could not read the browser history: {e}"))?;
    let pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
    let mut statement = connection
        .prepare(
            "SELECT url, title, last_visit_time, visit_count FROM urls \
             WHERE hidden = 0 AND (url LIKE ?1 ESCAPE '\\' OR title LIKE ?1 ESCAPE '\\') \
             ORDER BY last_visit_time DESC LIMIT ?2",
        )
        .map_err(|e| format!("could not read the browser history: {e}"))?;
    let rows = statement
        .query_map(rusqlite::params![pattern, limit as i64], |row| {
            let url: String = row.get(0)?;
            let title: String = row.get(1)?;
            let visited: i64 = row.get(2)?;
            let visits: i64 = row.get(3)?;
            Ok(json!({"url": url, "title": title, "last_visit": unix_from_chrome_time(visited), "visits": visits}))
        })
        .map_err(|e| format!("could not read the browser history: {e}"))?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// Seconds since the Unix epoch for the start of a range (`hour`, `day`, `week`, `all`).
fn range_start(range: &str, now: i64) -> Result<Option<i64>, String> {
    match range {
        "hour" => Ok(Some(now - 3_600)),
        "day" => Ok(Some(now - 86_400)),
        "week" => Ok(Some(now - 7 * 86_400)),
        "all" => Ok(None),
        other => Err(format!(
            "range must be hour, day, week or all, not '{other}'"
        )),
    }
}

/// Clears `types` newer than `since` (all when `None`) in one closed profile; returns what
/// was removed.
fn clear_profile(profile: &Path, since: Option<i64>, types: &[String]) -> Result<Value, String> {
    let cutoff = since.map(chrome_time_from_unix).unwrap_or(0);
    let mut cleared = serde_json::Map::new();
    if types.iter().any(|t| t == "history") {
        let path = profile.join("Default").join("History");
        if path.exists() {
            let connection = rusqlite::Connection::open(&path).map_err(|e| e.to_string())?;
            let visits = connection
                .execute("DELETE FROM visits WHERE visit_time >= ?1", [cutoff])
                .map_err(|e| e.to_string())?;
            connection
                .execute("DELETE FROM urls WHERE last_visit_time >= ?1 AND id NOT IN (SELECT url FROM visits)", [cutoff])
                .map_err(|e| e.to_string())?;
            cleared.insert("history_visits".into(), json!(visits));
        }
    }
    if types.iter().any(|t| t == "cookies") {
        let path = cookies_db(profile);
        if path.exists() {
            let connection = rusqlite::Connection::open(&path).map_err(|e| e.to_string())?;
            let cookies = connection
                .execute("DELETE FROM cookies WHERE creation_utc >= ?1", [cutoff])
                .map_err(|e| e.to_string())?;
            cleared.insert("cookies".into(), json!(cookies));
        }
        // Site storage has no per-entry time: it goes only with "all time".
        if since.is_none() {
            for folder in [
                "Local Storage",
                "Session Storage",
                "IndexedDB",
                "Service Worker",
                "File System",
                "WebStorage",
            ] {
                let _ = std::fs::remove_dir_all(profile.join("Default").join(folder));
            }
            cleared.insert("site_storage".into(), json!("all"));
        }
    }
    if types.iter().any(|t| t == "cache") {
        for folder in ["Cache", "Code Cache", "GPUCache"] {
            let _ = std::fs::remove_dir_all(profile.join("Default").join(folder));
        }
        cleared.insert("cache".into(), json!("all"));
    }
    Ok(Value::Object(cleared))
}

impl AppState {
    /// Project roots that have a browser profile, with the profile folder.
    async fn browser_profiles(
        &self,
        project_root: Option<&str>,
    ) -> Result<Vec<(PathBuf, PathBuf)>, String> {
        let hub = self.browser_hub();
        let roots: Vec<PathBuf> = match project_root {
            Some(root) => vec![PathBuf::from(root)],
            None => {
                let threads = self.inner.threads.read().await;
                let mut roots: Vec<PathBuf> =
                    threads.values().map(|t| t.project_root.clone()).collect();
                roots.sort();
                roots.dedup();
                roots
            }
        };
        Ok(roots
            .into_iter()
            .map(|root| {
                let profile = hub.profile_dir(&root);
                (root, profile)
            })
            .filter(|(_, profile)| profile.exists())
            .collect())
    }

    /// Refuses while a run uses `root`'s browser; otherwise closes that browser.
    async fn take_browser(&self, root: &Path) -> Result<(), String> {
        let busy = self
            .inner
            .threads
            .read()
            .await
            .values()
            .any(|thread| thread.running && thread.project_root == root);
        if busy {
            return Err(format!(
                "a run is using the browser of {}; stop it or wait until it ends",
                root.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("this project")
            ));
        }
        self.browser_hub().close_project(root).await;
        Ok(())
    }

    /// The built-in browser's history (newest first), for one project or all, filtered by
    /// `query` in the URL or title.
    pub async fn browser_history(
        &self,
        project_root: Option<String>,
        query: String,
    ) -> Result<Value, String> {
        let mut entries = Vec::new();
        for (root, profile) in self.browser_profiles(project_root.as_deref()).await? {
            for mut entry in read_history(&profile, query.trim(), HISTORY_LIMIT)? {
                entry["project"] = json!(root.to_string_lossy());
                entries.push(entry);
            }
        }
        entries.sort_by_key(|entry| std::cmp::Reverse(entry["last_visit"].as_i64().unwrap_or(0)));
        entries.truncate(HISTORY_LIMIT);
        Ok(json!({"entries": entries}))
    }

    /// Removes every visit of `url` from `project_root`'s browser history.
    pub async fn remove_browser_history(
        &self,
        project_root: String,
        url: String,
    ) -> Result<(), String> {
        let root = PathBuf::from(&project_root);
        self.take_browser(&root).await?;
        let path = self
            .browser_hub()
            .profile_dir(&root)
            .join("Default")
            .join("History");
        if !path.exists() {
            return Ok(());
        }
        let connection = rusqlite::Connection::open(&path).map_err(|e| e.to_string())?;
        connection
            .execute(
                "DELETE FROM visits WHERE url IN (SELECT id FROM urls WHERE url = ?1)",
                [&url],
            )
            .map_err(|e| e.to_string())?;
        connection
            .execute("DELETE FROM urls WHERE url = ?1", [&url])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Clears browsing data of `types` (`history`, `cookies` with site data, `cache`) from
    /// `range` (`hour`, `day`, `week`, `all`) for one project or all.
    pub async fn clear_browsing_data(
        &self,
        project_root: Option<String>,
        range: String,
        types: Vec<String>,
    ) -> Result<Value, String> {
        let unknown: Vec<&String> = types
            .iter()
            .filter(|t| !matches!(t.as_str(), "history" | "cookies" | "cache"))
            .collect();
        if types.is_empty() || !unknown.is_empty() {
            return Err("types must be one or more of history, cookies and cache".into());
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let since = range_start(&range, now)?;
        let profiles = self.browser_profiles(project_root.as_deref()).await?;
        for (root, _) in &profiles {
            self.take_browser(root).await?;
        }
        let mut cleared = Vec::new();
        for (root, profile) in profiles {
            let mut result = clear_profile(&profile, since, &types)?;
            result["project"] = json!(root.to_string_lossy());
            cleared.push(result);
        }
        let _ = self.inner.diagnostics.append(
            "browser",
            "data_cleared",
            "info",
            json!({"range": range, "types": types, "projects": cleared.len()}),
        );
        Ok(json!({"cleared": cleared}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history_db(profile: &Path) -> rusqlite::Connection {
        std::fs::create_dir_all(profile.join("Default")).unwrap();
        let connection =
            rusqlite::Connection::open(profile.join("Default").join("History")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE urls(id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER, last_visit_time INTEGER, hidden INTEGER DEFAULT 0);
                 CREATE TABLE visits(id INTEGER PRIMARY KEY, url INTEGER, visit_time INTEGER);",
            )
            .unwrap();
        connection
    }

    #[test]
    fn history_reads_newest_first_and_clearing_respects_the_range() {
        let profile = tempfile::tempdir().unwrap();
        let connection = history_db(profile.path());
        let (old, recent) = (
            chrome_time_from_unix(1_000),
            chrome_time_from_unix(2_000_000_000),
        );
        connection
            .execute(
                "INSERT INTO urls VALUES (1, 'http://localhost:5173/', 'Chess', 3, ?1, 0)",
                [recent],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO urls VALUES (2, 'https://docs.rs/serde', 'serde', 1, ?1, 0)",
                [old],
            )
            .unwrap();
        connection
            .execute("INSERT INTO visits VALUES (1, 1, ?1)", [recent])
            .unwrap();
        connection
            .execute("INSERT INTO visits VALUES (2, 2, ?1)", [old])
            .unwrap();
        drop(connection);
        let all = read_history(profile.path(), "", 10).unwrap();
        assert_eq!(all[0]["title"], "Chess");
        assert_eq!(all[0]["last_visit"], 2_000_000_000);
        assert_eq!(read_history(profile.path(), "docs", 10).unwrap().len(), 1);
        assert_eq!(
            read_history(profile.path(), "100%", 10).unwrap().len(),
            0,
            "% is literal"
        );
        // Clearing the last week removes the recent visit only.
        let cleared =
            clear_profile(profile.path(), Some(1_999_999_000), &["history".into()]).unwrap();
        assert_eq!(cleared["history_visits"], 1);
        let left = read_history(profile.path(), "", 10).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0]["title"], "serde");
        assert!(range_start("month", 0).is_err());
        assert_eq!(range_start("all", 5).unwrap(), None);
    }

    #[test]
    fn clearing_cache_and_all_time_site_data_removes_their_folders() {
        let profile = tempfile::tempdir().unwrap();
        for folder in ["Cache", "Local Storage", "IndexedDB"] {
            std::fs::create_dir_all(profile.path().join("Default").join(folder)).unwrap();
        }
        clear_profile(profile.path(), None, &["cache".into(), "cookies".into()]).unwrap();
        for folder in ["Cache", "Local Storage", "IndexedDB"] {
            assert!(
                !profile.path().join("Default").join(folder).exists(),
                "{folder}"
            );
        }
    }
}
