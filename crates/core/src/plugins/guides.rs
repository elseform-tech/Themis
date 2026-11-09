//! Which built-in integration guides a person's message is about, by a plain word match: the
//! turn then gets those guides as if the person had picked them (`/mcp`, `/plugins`, `/skills`,
//! `/hooks`), without the model having to load them. No model call; a stray match only costs
//! the guide's text in that turn.

/// The guides, in the order they are attached, with the words that call for each. Words are
/// whole (case-insensitive) runs of letters and digits, so `.mcp.json`, `mcp-server` and
/// `SKILL.md` match and `hooked` or `skillful` do not.
const GUIDES: &[(&str, &[&str])] = &[
    ("mcp", &["mcp", "mcps"]),
    (
        "plugins",
        &["plugin", "plugins", "marketplace", "marketplaces"],
    ),
    ("skills", &["skill", "skills"]),
    ("hooks", &["hook", "hooks"]),
];

/// The ids of the integration guides `message` is about, each once, in [`GUIDES`] order.
/// Capability references (`[[skill:…]]`, `[[plugin:…]]`) are not words of the message.
#[must_use]
pub fn integration_guides(message: &str) -> Vec<&'static str> {
    let mut words = std::collections::HashSet::new();
    let mut rest = message;
    let mut text = String::with_capacity(message.len());
    while let Some(start) = rest.find("[[") {
        text.push_str(&rest[..start]);
        match rest[start..].find("]]") {
            Some(end) => rest = &rest[start + end + 2..],
            None => {
                rest = &rest[start + 2..];
            }
        }
    }
    text.push_str(rest);
    for word in text.split(|c: char| !c.is_alphanumeric()) {
        if !word.is_empty() {
            words.insert(word.to_lowercase());
        }
    }
    GUIDES
        .iter()
        .filter(|(_, calls)| calls.iter().any(|call| words.contains(*call)))
        .map(|(id, _)| *id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_about_integrations_call_their_guides() {
        let cases: &[(&str, &[&str])] = &[
            ("add an MCP server called notes", &["mcp"]),
            ("put this .mcp.json into Themis", &["mcp"]),
            ("set up the mcp-server for docs", &["mcp"]),
            (
                "install the ralph plugin from the marketplace",
                &["plugins"],
            ),
            ("Marketplaces?", &["plugins"]),
            ("write a SKILL.md for release notes", &["skills"]),
            ("make a skill and a hook", &["skills", "hooks"]),
            ("Hooks: remove the noisy one", &["hooks"]),
            (
                "an MCP plugin with skills and hooks",
                &["mcp", "plugins", "skills", "hooks"],
            ),
        ];
        for (message, expected) in cases {
            assert_eq!(integration_guides(message), *expected, "{message}");
        }
    }

    #[test]
    fn other_words_call_none() {
        for message in [
            "fix the failing test",
            "I'm hooked on this design",
            "a skillful refactor",
            "mcpx is a different tool",
            "unplugged",
            "",
            "[[skill:mcp]] go",
            "[[plugin:g--hooks]] run it",
        ] {
            assert!(integration_guides(message).is_empty(), "{message}");
        }
        // A reference does not hide the words around it.
        assert_eq!(integration_guides("[[skill:review]] add a hook"), ["hooks"]);
        // An unfinished reference is text.
        assert_eq!(integration_guides("[[ mcp"), ["mcp"]);
    }
}
