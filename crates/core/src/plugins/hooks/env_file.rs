use super::*;

/// Most of an env file read, and how many variables are taken from it.
const ENV_FILE_BYTES: u64 = 64 * 1024;
const ENV_FILE_VARIABLES: usize = 64;

/// One plugin's `CLAUDE_ENV_FILE`. What its session hooks write there reaches its own later hooks
/// and nothing else: not the agent's commands, and not another plugin. Those hooks already run
/// with the person's permissions, so this adds nothing to what the plugin can do.
///
/// A list of dangerous names can't be complete (`GCONV_PATH`, `PERL5LIB`, `RUBYLIB`,
/// `JAVA_TOOL_OPTIONS`, `npm_config_*`, …), so the rule says what is allowed: names under the
/// plugin's own prefix, in capitals (`my-kit` may set `MY_KIT_ANYTHING`). In the official catalog
/// the one plugin whose hooks write this file is `vercel`, with `VERCEL_PLUGIN_*` names it reads
/// back in its own later hooks, so this fits what ships.
#[derive(Clone, Debug)]
pub struct PluginEnvFile {
    pub path: std::path::PathBuf,
    /// Its variables' prefix, from [`env_prefix`].
    pub prefix: String,
}

/// The prefix a plugin's variables must have: its name in capitals, then `_`.
#[must_use]
pub fn env_prefix(plugin: &str) -> String {
    let name: String = plugin
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("{name}_")
}

/// Whether a plugin may set this name: its prefix, then at least one more character, all capitals,
/// digits and `_`. A name a shell couldn't use, or one in lower case, is not confirmed.
pub(super) fn env_name_allowed(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .is_some_and(|rest| !rest.is_empty())
        && name
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

/// The variables in an env file, and the names refused. It is read, never run: `export NAME=value`
/// and `NAME=value`, with a plain or quoted value. A value that would need a shell to mean anything
/// is refused with its name.
pub(super) fn parse_env_file(text: &str, prefix: &str) -> (Vec<(String, String)>, Vec<String>) {
    let mut variables: Vec<(String, String)> = Vec::new();
    let mut refused: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let plain = ['"', '\'']
            .into_iter()
            .find_map(|quote| value.strip_prefix(quote)?.strip_suffix(quote))
            .unwrap_or(value);
        let needs_a_shell = plain.contains([
            '$', '`', '\\', ';', '|', '&', '<', '>', '(', ')', '\n', '"', '\'',
        ]) || (plain.len() == value.len()
            && plain.contains(char::is_whitespace));
        if env_name_allowed(name, prefix) && !needs_a_shell && variables.len() < ENV_FILE_VARIABLES
        {
            variables.retain(|(known, _)| known != name);
            variables.push((name.to_owned(), plain.to_owned()));
        } else if refused.len() < ENV_FILE_VARIABLES {
            // Only what can be shown safely: what a hook wrote is not to be trusted as text.
            let shown: String = name
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                .take(64)
                .collect();
            if !shown.is_empty() && !refused.contains(&shown) {
                refused.push(shown);
            }
        }
    }
    (variables, refused)
}

impl PluginEnvFile {
    /// What it allows, and the names it refuses.
    pub(super) fn read(&self) -> (Vec<(String, String)>, Vec<String>) {
        let mut text = String::new();
        let read = std::fs::File::open(&self.path)
            .and_then(|file| file.take(ENV_FILE_BYTES).read_to_string(&mut text));
        match read {
            Ok(_) => parse_env_file(&text, &self.prefix),
            Err(_) => (Vec::new(), Vec::new()),
        }
    }
}
