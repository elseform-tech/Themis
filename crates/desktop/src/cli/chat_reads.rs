//! `themis recall` and `themis memory search|show`: a chat's saved originals and what Themis
//! remembers in its project, through the app server's `recall` and `search_memory` methods (the
//! same work and bounded results as the built-in tools). In an agent's shell the server binds
//! them to the run's own chat, so `--thread` may be left out; anywhere else it names the chat.

use crate::server::Client;
use serde_json::{json, Map, Value};

pub(super) const RECALL_USAGE: &str =
    "usage: themis recall [--thread ID] [--limit N] [--source TEXT] [--kind KIND]... KEYWORDS...
       themis recall [--thread ID] --around TOKEN
Searches the chat's saved originals (earlier messages, tool output and attachments that left
context) and prints ranked excerpts as JSON; a hit's `around` value reads more around it.";

pub(super) const MEMORY_USAGE: &str = "usage: themis memory search [--thread ID] WORDS...
       themis memory show [--thread ID] NAME
Searches what Themis remembers (this project's and the global memories, plan records and
tasks), or shows one memory or plan record in full (`tasks` shows every task). Read-only.";

/// The chat a call names: `--thread ID`, or in an agent's shell its own (filled in by the server).
fn thread(
    fields: &mut Map<String, Value>,
    thread: Option<String>,
    usage: &str,
) -> Result<(), String> {
    match thread {
        Some(id) => {
            fields.insert("threadId".into(), json!(id));
            Ok(())
        }
        None if crate::server::in_agent_shell() => Ok(()),
        None => Err(format!("pass --thread ID\n{usage}")),
    }
}

/// The value after option `words[*index]`, moving past both.
fn value(words: &[String], index: &mut usize, usage: &str) -> Result<String, String> {
    let found = words
        .get(*index + 1)
        .cloned()
        .ok_or_else(|| usage.to_owned())?;
    *index += 2;
    Ok(found)
}

/// `themis recall …` as the `recall` method's arguments.
fn recall_args(words: &[String]) -> Result<Value, String> {
    let usage = RECALL_USAGE;
    let mut fields = Map::new();
    let (mut chat, mut kinds, mut query) = (None, Vec::new(), Vec::new());
    let mut index = 0;
    while let Some(word) = words.get(index) {
        match word.as_str() {
            "--thread" => chat = Some(value(words, &mut index, usage)?),
            "--limit" => {
                let limit = value(words, &mut index, usage)?;
                let limit: u64 = limit
                    .parse()
                    .map_err(|_| format!("--limit takes a number\n{usage}"))?;
                fields.insert("limit".into(), json!(limit));
            }
            "--source" => {
                fields.insert("source".into(), json!(value(words, &mut index, usage)?));
            }
            "--kind" => kinds.push(value(words, &mut index, usage)?),
            "--around" => {
                fields.insert("around".into(), json!(value(words, &mut index, usage)?));
            }
            "--" => {
                query.extend(words[index + 1..].iter().cloned());
                break;
            }
            option if option.starts_with("--") => return Err(usage.to_owned()),
            _ => {
                query.push(word.clone());
                index += 1;
            }
        }
    }
    if query.is_empty() && !fields.contains_key("around") {
        return Err(usage.to_owned());
    }
    fields.insert("query".into(), json!(query.join(" ")));
    if !kinds.is_empty() {
        fields.insert("kinds".into(), json!(kinds));
    }
    thread(&mut fields, chat, usage)?;
    Ok(Value::Object(fields))
}

/// `themis memory search|show …` as the `search_memory` method's arguments.
fn memory_args(words: &[String]) -> Result<Value, String> {
    let usage = MEMORY_USAGE;
    let (verb, rest) = words.split_first().ok_or_else(|| usage.to_owned())?;
    let mut chat = None;
    let mut text = Vec::new();
    let mut index = 0;
    while let Some(word) = rest.get(index) {
        if word == "--thread" {
            chat = Some(value(rest, &mut index, usage)?);
        } else if word.starts_with("--") {
            return Err(usage.to_owned());
        } else {
            text.push(word.clone());
            index += 1;
        }
    }
    let field = match verb.as_str() {
        "search" if !text.is_empty() => "query",
        "show" if text.len() == 1 => "name",
        _ => return Err(usage.to_owned()),
    };
    let mut fields = Map::new();
    fields.insert(field.into(), json!(text.join(" ")));
    thread(&mut fields, chat, usage)?;
    Ok(Value::Object(fields))
}

/// Runs `themis recall …` or `themis memory …` (`command`, then the `words` after it).
pub(super) async fn run(client: &Client, command: &str, words: &[String]) -> Result<Value, String> {
    if command == "recall" {
        client.call("recall", recall_args(words)?).await
    } else {
        client.call("search_memory", memory_args(words)?).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn recall_words_become_the_tools_arguments() {
        let args = recall_args(&words(
            "--thread t1 --limit 3 --kind original --kind recovery --source User dock code",
        ))
        .unwrap();
        assert_eq!(
            args,
            json!({"threadId": "t1", "limit": 3, "source": "User",
                "kinds": ["original", "recovery"], "query": "dock code"})
        );
        let around = recall_args(&words("--thread t1 --around a|b|c")).unwrap();
        assert_eq!(
            around,
            json!({"threadId": "t1", "around": "a|b|c", "query": ""})
        );
        let dashed = recall_args(&words("--thread t1 -- --limit words")).unwrap();
        assert_eq!(dashed["query"], "--limit words");
        for wrong in [
            "--thread t1",
            "--thread t1 --limit x a",
            "--thread t1 --bogus a",
            "--thread",
        ] {
            assert!(recall_args(&words(wrong)).is_err(), "{wrong}");
        }
    }

    #[test]
    fn memory_words_become_the_tools_arguments() {
        assert_eq!(
            memory_args(&words("search --thread t1 release steps")).unwrap(),
            json!({"threadId": "t1", "query": "release steps"})
        );
        assert_eq!(
            memory_args(&words("show tasks --thread t1")).unwrap(),
            json!({"threadId": "t1", "name": "tasks"})
        );
        for wrong in [
            "",
            "search --thread t1",
            "show a b --thread t1",
            "delete m --thread t1",
        ] {
            assert!(memory_args(&words(wrong)).is_err(), "{wrong}");
        }
    }
}
