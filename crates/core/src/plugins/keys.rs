//! Keys that MCP servers reference as `${NAME}`: saved in Settings › Keys (the OS
//! credential store, through the same store as sign-ins), or else an environment variable.

use anyhow::bail;

/// Where a saved key named `name` is kept in the credential store.
pub fn store_key(name: &str) -> String {
    format!("key:{name}")
}

/// A key name: letters, digits and `_`, as environment variables are named.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Whether a key named `name` is saved in Settings › Keys.
pub fn saved(name: &str) -> bool {
    super::oauth::load_entry(&store_key(name)).is_some_and(|value| !value.is_empty())
}

/// The value for `${name}`: the saved key, else the environment variable of that name.
pub fn value(name: &str) -> Option<String> {
    super::oauth::load_entry(&store_key(name))
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var(name).ok().filter(|value| !value.is_empty()))
}

/// Splits a header template `prefix${NAME}` (such as `Bearer ${GITHUB_TOKEN}`) into its
/// literal prefix and key name. The key comes last and appears once; the prefix has no `$`.
pub fn header_template(template: &str) -> anyhow::Result<(&str, &str)> {
    let Some((prefix, rest)) = template.split_once("${") else {
        bail!(
            "MCP header values must reference a key as ${{NAME}}; literal values are unsupported"
        );
    };
    let name = rest.strip_suffix('}').unwrap_or_default();
    if prefix.contains('$')
        || !prefix.bytes().all(|b| b == b' ' || b.is_ascii_graphic())
        || !valid_name(name)
    {
        bail!("MCP header values must be an optional prefix followed by one ${{NAME}}");
    }
    Ok((prefix, name))
}

/// A whole value that names a key, as imported configurations write it: `${NAME}`,
/// `${NAME:-default}`, `${env:NAME}`, `{env:NAME}`, or Claude's install-time setting
/// `${user_config.option}` (read as the key `OPTION`). Returns the key and its default.
pub fn reference(value: &str) -> Option<(String, Option<String>)> {
    let inner = value
        .strip_prefix("${")
        .or_else(|| value.strip_prefix("{env:"))?
        .strip_suffix('}')?;
    let (name, default) = match inner.split_once(":-") {
        Some((name, default)) if value.starts_with("${") => (name, Some(default.to_owned())),
        _ => (inner, None),
    };
    let name = name.strip_prefix("env:").unwrap_or(name);
    let name = match name.strip_prefix("user_config.") {
        Some(option) => option.to_ascii_uppercase(),
        None => name.to_owned(),
    };
    valid_name(&name).then_some((name, default))
}

/// Rewrites every reference inside `text` (a URL or header template) to the plain `${NAME}`
/// form, returning the defaults it carried (`${NAME:-value}`); `None` when one is malformed.
pub fn normalize_references(text: &str) -> Option<(String, Vec<(String, String)>)> {
    let text = text.replace("${env:", "${").replace("{env:", "${");
    let mut out = String::new();
    let mut defaults = Vec::new();
    let mut rest = text.as_str();
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let end = rest[start..].find('}')? + start;
        let (name, default) = reference(&rest[start..=end])?;
        out.push_str(&format!("${{{name}}}"));
        defaults.extend(default.map(|default| (name, default)));
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Some((out, defaults))
}

/// The key names a template such as `${BASE}/mcp` reads, in order.
pub fn template_names(text: &str) -> Vec<&str> {
    text.split("${")
        .skip(1)
        .filter_map(|part| part.split_once('}').map(|(name, _)| name))
        .collect()
}

/// `text` with every `${NAME}` replaced by `value(NAME)`.
pub fn fill(text: &str, value: impl Fn(&str) -> String) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some((before, after)) = rest.split_once("${") {
        out.push_str(before);
        match after.split_once('}') {
            Some((name, tail)) => {
                out.push_str(&value(name));
                rest = tail;
            }
            None => {
                out.push_str("${");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Whether a setting or header name suggests it holds a credential.
pub fn secret_name(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    [
        "KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "PWD",
        "AUTH",
        "CREDENTIAL",
        "COOKIE",
        "SESSION",
        "PRIVATE",
        "CERT",
        "SIGNATURE",
    ]
    .iter()
    .any(|word| name.contains(word))
}

/// Whether `value` may be saved as plain text for the setting `name`: never for names that
/// suggest a credential, and only short printable values that don't look like a token
/// (a long run mixing letters and digits) or carry a user name or password in a URL.
pub fn safe_literal(name: &str, value: &str) -> bool {
    if secret_name(name)
        || value.len() > 256
        || !value.bytes().all(|b| (b' '..=b'~').contains(&b))
        || value.contains("${")
        || value.contains("{env:")
    {
        return false;
    }
    let token_like = value
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|part| {
            part.len() >= 20
                && part.bytes().any(|b| b.is_ascii_digit())
                && part.bytes().any(|b| b.is_ascii_alphabetic())
        });
    let url_credentials = value.contains("://")
        && reqwest::Url::parse(value)
            .is_ok_and(|url| !url.username().is_empty() || url.password().is_some());
    !token_like && !url_credentials
}

/// The plugin's own folder, filled in when a connection starts (never a key).
pub const PLUGIN_ROOTS: [&str; 2] = ["${CLAUDE_PLUGIN_ROOT}", "${PLUGIN_ROOT}"];

/// [`safe_literal`] for a value that may also name the plugin's folder.
pub fn safe_plugin_value(name: &str, value: &str) -> bool {
    let without_root = PLUGIN_ROOTS
        .iter()
        .fold(value.to_owned(), |text, root| text.replace(root, ""));
    safe_literal(name, &without_root)
}

/// The error a connection reports when keys are missing, naming them (never values).
pub fn missing_error(names: &[String]) -> anyhow::Error {
    anyhow::anyhow!("{NEEDS_KEY}{}", names.join(", "))
}

/// The start of a missing-key error; the names follow it, comma separated.
pub const NEEDS_KEY: &str = "MCP key needed: ";

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn header_templates_take_a_prefix_and_one_key() {
        assert_eq!(header_template("Bearer ${GH}").unwrap(), ("Bearer ", "GH"));
        assert_eq!(header_template("${API_KEY}").unwrap(), ("", "API_KEY"));
        for bad in [
            "sk-123", "${A}${B}", "x${A} y", "$x${A}", "${a-b}", "${}", "${A",
        ] {
            assert!(header_template(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn references_take_every_imported_form() {
        let key = |name: &str| Some((name.to_owned(), None));
        assert_eq!(reference("${API_KEY}"), key("API_KEY"));
        assert_eq!(reference("${env:API_KEY}"), key("API_KEY"));
        assert_eq!(reference("{env:API_KEY}"), key("API_KEY"));
        assert_eq!(
            reference("${user_config.bigquery_project}"),
            key("BIGQUERY_PROJECT")
        );
        assert_eq!(
            reference("${URL:-http://localhost:3000}"),
            Some(("URL".into(), Some("http://localhost:3000".into())))
        );
        for bad in [
            "API_KEY",
            "${a-b}",
            "${}",
            "${A",
            "prefix ${A}",
            "{env:A:-x}",
        ] {
            assert_eq!(reference(bad), None, "{bad}");
        }
        let plain = |text: &str| normalize_references(text).map(|(text, _)| text);
        assert_eq!(
            plain("${env:BASE}/mcp/{env:PATH}").as_deref(),
            Some("${BASE}/mcp/${PATH}")
        );
        assert_eq!(
            plain("Bearer ${env:TOKEN}").as_deref(),
            Some("Bearer ${TOKEN}")
        );
        assert_eq!(
            normalize_references("${URL:-https://logfire-us.pydantic.dev/mcp}"),
            Some((
                "${URL}".into(),
                vec![("URL".into(), "https://logfire-us.pydantic.dev/mcp".into())]
            ))
        );
        assert_eq!(normalize_references("${a-b}/mcp"), None);
        assert_eq!(template_names("${BASE}/mcp?x=${B}"), ["BASE", "B"]);
        assert_eq!(
            fill("${BASE}/mcp", |name| format!("<{name}>")),
            "<BASE>/mcp"
        );
    }

    #[test]
    fn plain_values_exclude_credentials() {
        for (name, value) in [
            ("FASTMCP_LOG_LEVEL", "ERROR"),
            ("x-exa-source", "claude-code-plugin"),
            ("User-Agent", "postman-factory-plugin/0.3.0"),
            (
                "X_GOOG_PROXY_HEADERS",
                "{\"User-Agent\":\"datacloud.other\"}",
            ),
            ("GRAFANA_URL", "http://localhost:3000"),
        ] {
            assert!(safe_literal(name, value), "{name}={value}");
        }
        for (name, value) in [
            ("API_KEY", "abc"),
            ("x-auth", "anything"),
            ("NOTE", "aZ8kQ2mX9pL4vB7nR3tY6wE1"),
            ("ENDPOINT", "https://user:pass@example.com"),
            ("REGION", "us-east-1 — required"),
            ("BASE", "${OTHER}"),
        ] {
            assert!(!safe_literal(name, value), "{name}={value}");
        }
    }

    proptest::proptest! {
        #[test]
        fn references_never_panic_and_name_valid_keys(text in "\\PC{0,60}") {
            if let Some((name, _)) = reference(&text) {
                proptest::prop_assert!(valid_name(&name));
            }
            let _ = normalize_references(&text);
            let _ = safe_literal("NAME", &text);
            let _ = fill(&text, |name| name.to_owned());
        }

        #[test]
        fn names_that_suggest_credentials_are_never_plain(prefix in "[A-Za-z_]{0,8}", word in proptest::sample::select(vec!["key", "TOKEN", "Secret", "password", "Auth"]), value in "[a-z]{1,10}") {
            let name = format!("{prefix}{word}");
            proptest::prop_assert!(!safe_literal(&name, &value));
        }

        #[test]
        fn any_text_parses_without_panicking(text in "\\PC{0,80}") {
            if let Ok((prefix, name)) = header_template(&text) {
                proptest::prop_assert_eq!(format!("{prefix}${{{name}}}"), text.as_str());
                proptest::prop_assert!(!prefix.contains('$') && valid_name(name));
            }
        }

        #[test]
        fn a_prefix_and_a_name_round_trip(prefix in "[ -#%-~]{0,20}", name in "[A-Za-z0-9_]{1,40}") {
            let template = format!("{prefix}${{{name}}}");
            proptest::prop_assert_eq!(header_template(&template).unwrap(), (prefix.as_str(), name.as_str()));
        }
    }
}
