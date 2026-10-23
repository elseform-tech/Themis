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

    proptest::proptest! {
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
