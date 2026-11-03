//! The looks check (`browser_check_looks`): what can be measured about how the current page
//! looks, without seeing it. `looks.js` runs once with the page emulating a light colour scheme
//! and once dark; problems found in both are listed once. Whole-page problems (quirks mode,
//! CSS rules the browser dropped, resources that failed to load, console errors) come first.
//! The page's own script environment produces the report, so it is untrusted: any shape is
//! read without panicking and every list and line is bounded.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use super::{cap, short_url, ChatBrowser, TabState};
use crate::browser::cdp::Cdp;
use crate::browser::redact;
use crate::browser::snapshot::RefMap;

const SCRIPT: &str = include_str!("../looks.js");
const SCHEMES: [&str; 2] = ["light", "dark"];
/// Kinds of element problems, in the order they are listed.
const KINDS: [&str; 5] = ["contrast", "control", "code", "overflow", "clipped"];
/// Items one kind contributes per scheme (the script keeps no more either).
const PER_KIND: usize = 15;
/// Lines one list of the result shows.
const LISTED: usize = 15;
const LINE_CHARS: usize = 240;
const LOADS_LISTED: usize = 8;
const CONSOLE_LISTED: usize = 5;
/// Request kinds whose failure changes how the page looks.
const LOOKS_RESOURCES: [&str; 4] = ["Stylesheet", "Script", "Image", "Font"];
const OBJECT_GROUP: &str = "themis-looks";
const CHECK_TIMEOUT: Duration = Duration::from_secs(25);

/// What `looks.js` reports for one scheme.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SchemeReport {
    checked: u64,
    capped: bool,
    // Read loosely: one odd entry costs that entry, not the report.
    page: Vec<Value>,
    dropped: Vec<Value>,
    items: BTreeMap<String, Value>,
    counts: BTreeMap<String, Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Item {
    text: String,
    describe: String,
    /// Index into the script's elements, or -1 for none.
    el: i64,
}

/// One scheme's findings as lines, elements named with refs.
#[derive(Debug, Default, PartialEq)]
struct SchemeLines {
    page: Vec<String>,
    elements: Vec<String>,
    /// Problems the script found but did not list.
    unlisted: u64,
    checked: u64,
    capped: bool,
}

/// Puts the page back in the colour scheme it had and frees the script's objects, also when
/// the check is stopped part way.
struct Restore {
    cdp: Arc<Cdp>,
    session: String,
}

impl Drop for Restore {
    fn drop(&mut self) {
        self.cdp.send(
            Some(&self.session),
            "Emulation.setEmulatedMedia",
            json!({"features": [{"name": "prefers-color-scheme", "value": ""}]}),
        );
        self.cdp.send(
            Some(&self.session),
            "Runtime.releaseObjectGroup",
            json!({"objectGroup": OBJECT_GROUP}),
        );
    }
}

/// Runs the script in one scheme; returns its report and each element's DOM node.
async fn run_scheme(
    cdp: &Cdp,
    session: &str,
    scheme: &str,
) -> Result<(Value, Vec<Option<i64>>), String> {
    cdp.call(
        Some(session),
        "Emulation.setEmulatedMedia",
        json!({"features": [{"name": "prefers-color-scheme", "value": scheme}]}),
    )
    .await?;
    let evaluated = cdp
        .call_with_timeout(
            Some(session),
            "Runtime.evaluate",
            json!({"expression": SCRIPT, "awaitPromise": true, "objectGroup": OBJECT_GROUP, "timeout": 20_000}),
            CHECK_TIMEOUT,
        )
        .await?;
    if let Some(details) = evaluated.get("exceptionDetails") {
        let text = details["exception"]["description"]
            .as_str()
            .or_else(|| details["text"].as_str())
            .unwrap_or("it threw");
        return Err(format!(
            "the check could not run on this page: {}",
            cap(text, 200)
        ));
    }
    let object = evaluated["result"]["objectId"]
        .as_str()
        .ok_or("the check returned nothing")?;
    let part = |name: &str, by_value: bool| {
        json!({
            "objectId": object,
            "functionDeclaration": format!("function(){{return this.{name}}}"),
            "returnByValue": by_value,
            "objectGroup": OBJECT_GROUP,
        })
    };
    let report = cdp
        .call(
            Some(session),
            "Runtime.callFunctionOn",
            part("report", true),
        )
        .await?["result"]["value"]
        .clone();
    let list = cdp
        .call(
            Some(session),
            "Runtime.callFunctionOn",
            part("elements", false),
        )
        .await?;
    let mut nodes = Vec::new();
    if let Some(list) = list["result"]["objectId"].as_str() {
        let properties = cdp
            .call(
                Some(session),
                "Runtime.getProperties",
                json!({"objectId": list, "ownProperties": true}),
            )
            .await?;
        for property in properties["result"].as_array().into_iter().flatten() {
            let (Some(index), Some(element)) = (
                property["name"]
                    .as_str()
                    .and_then(|n| n.parse::<usize>().ok()),
                property["value"]["objectId"].as_str(),
            ) else {
                continue;
            };
            // The script keeps at most this many; a page that changed it gets no more refs.
            if index >= KINDS.len() * PER_KIND {
                continue;
            }
            if nodes.len() <= index {
                nodes.resize(index + 1, None);
            }
            nodes[index] = cdp
                .call(
                    Some(session),
                    "DOM.describeNode",
                    json!({"objectId": element}),
                )
                .await
                .ok()
                .and_then(|node| node["node"]["backendNodeId"].as_i64());
        }
    }
    Ok((report, nodes))
}

/// `text` with every URL's query and fragment cut off (they can carry tokens), and token-like
/// words hidden unless Show secrets is on. Every page-supplied string goes through this.
fn shown(text: &str, show_secrets: bool) -> String {
    let text: String = text
        .split_inclusive(char::is_whitespace)
        .map(|word| match (word.contains("://"), word.find(['?', '#'])) {
            (true, Some(end)) => {
                let space = &word[word.trim_end().len()..];
                format!("{}{space}", &word[..end])
            }
            _ => word.to_owned(),
        })
        .collect();
    if show_secrets {
        text
    } else {
        redact::redact_text(&text)
    }
}

/// One scheme's report as lines; elements get the snapshot's refs (new ones when the snapshot
/// had none for them).
fn lines_of(
    report: Value,
    nodes: &[Option<i64>],
    refs: &mut RefMap,
    show_secrets: bool,
) -> Result<SchemeLines, String> {
    let report: SchemeReport = serde_json::from_value(report)
        .map_err(|_| "the page's check returned something unreadable".to_owned())?;
    let mut lines = SchemeLines {
        checked: report.checked,
        capped: report.capped,
        ..SchemeLines::default()
    };
    lines.page = report
        .page
        .iter()
        .chain(&report.dropped)
        .filter_map(Value::as_str)
        .take(LISTED)
        .map(|line| cap(&shown(line, show_secrets), LINE_CHARS))
        .collect();
    for kind in KINDS {
        let items: Vec<Item> = report
            .items
            .get(kind)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(PER_KIND)
            .filter_map(|item| Item::deserialize(item).ok())
            .collect();
        let listed = items.len();
        for item in &items {
            let node = usize::try_from(item.el)
                .ok()
                .and_then(|index| nodes.get(index).copied().flatten());
            let what = cap(&shown(&item.describe, show_secrets), 80);
            let text = cap(&shown(&item.text, show_secrets), 140);
            let line = match node {
                Some(node) => format!("{text}: {what} [e{}]", refs.assign(node)),
                None => format!("{text}: {what}"),
            };
            lines.elements.push(line);
        }
        let found = report.counts.get(kind).and_then(Value::as_u64).unwrap_or(0);
        lines.unlisted = lines
            .unlisted
            .saturating_add(found.saturating_sub(listed as u64));
    }
    Ok(lines)
}

/// Resources of the current page that failed to load, and its console errors.
fn page_log(tab: &TabState, show_secrets: bool) -> Vec<String> {
    let safe = |text: &str| shown(text, show_secrets);
    let failed: Vec<String> = tab
        .requests
        .iter()
        .filter(|request| request.seq > tab.document_seq)
        .filter(|request| LOOKS_RESOURCES.contains(&request.kind.as_str()))
        .filter_map(|request| {
            // A failed status also aborts the load; the status says more.
            let why = match (request.status, &request.failed) {
                (Some(status), _) if status >= 400 => format!("HTTP {status}"),
                (_, Some(error)) => cap(error, 60),
                _ => return None,
            };
            Some(format!(
                "{} failed to load: {} ({why})",
                request.kind.to_lowercase(),
                safe(&short_url(&request.url))
            ))
        })
        .collect();
    let errors: Vec<String> = tab
        .console
        .iter()
        .filter(|entry| entry.seq > tab.document_seq)
        .filter(|entry| matches!(entry.level.as_str(), "error" | "exception" | "assert"))
        // The failed request is listed already.
        .filter(|entry| !entry.text.starts_with("Failed to load resource"))
        .map(|entry| format!("console error: {}", cap(&safe(&entry.text), 200)))
        .collect();
    let mut log = Vec::new();
    for (lines, limit, what) in [
        (failed, LOADS_LISTED, "failed loads"),
        (errors, CONSOLE_LISTED, "console errors"),
    ] {
        let more = lines.len().saturating_sub(limit);
        log.extend(lines.into_iter().take(limit));
        if more > 0 {
            log.push(format!("and {more} more {what}"));
        }
    }
    log
}

/// The tool result: whole-page problems, then element problems in both schemes, light only
/// and dark only, each list bounded.
fn merge(light: SchemeLines, dark: SchemeLines, log: Vec<String>) -> Value {
    fn unique(lines: impl IntoIterator<Item = String>) -> Vec<String> {
        let mut seen = HashSet::new();
        lines
            .into_iter()
            .filter(|line| seen.insert(line.clone()))
            .collect()
    }
    let (light_page, dark_page): (HashSet<_>, HashSet<_>) = (
        light.page.iter().cloned().collect(),
        dark.page.iter().cloned().collect(),
    );
    let mut page = unique(light.page.iter().map(|line| {
        if dark_page.contains(line) {
            line.clone()
        } else {
            format!("light mode: {line}")
        }
    }));
    page.extend(unique(
        dark.page
            .iter()
            .filter(|line| !light_page.contains(*line))
            .map(|line| format!("dark mode: {line}")),
    ));
    page.extend(log);
    let (light_set, dark_set): (HashSet<_>, HashSet<_>) = (
        light.elements.iter().cloned().collect(),
        dark.elements.iter().cloned().collect(),
    );
    let both = unique(
        light
            .elements
            .iter()
            .filter(|l| dark_set.contains(*l))
            .cloned(),
    );
    let light_only = unique(
        light
            .elements
            .iter()
            .filter(|l| !dark_set.contains(*l))
            .cloned(),
    );
    let dark_only = unique(
        dark.elements
            .iter()
            .filter(|l| !light_set.contains(*l))
            .cloned(),
    );
    let unlisted = light.unlisted.max(dark.unlisted);
    let problems = (page.len() + both.len() + light_only.len() + dark_only.len()) as u64 + unlisted;
    let mut result = json!({"problems": problems});
    let mut not_shown = unlisted;
    for (name, lines) in [
        ("page", page),
        ("both", both),
        ("light_only", light_only),
        ("dark_only", dark_only),
    ] {
        if lines.is_empty() {
            continue;
        }
        not_shown += lines.len().saturating_sub(LISTED) as u64;
        result[name] = json!(lines.into_iter().take(LISTED).collect::<Vec<_>>());
    }
    if not_shown > 0 {
        result["not_shown"] = json!(not_shown);
    }
    let checked = light.checked.max(dark.checked);
    result["checked"] = json!(if light.capped || dark.capped {
        format!("the first {checked} elements, in light and dark")
    } else {
        format!("{checked} elements, in light and dark")
    });
    result
}

impl ChatBrowser {
    /// Checks how the current page looks in light and dark colour schemes and puts the page
    /// back in the scheme it had.
    pub async fn check_looks(self: &Arc<Self>, show_secrets: bool) -> Result<Value, String> {
        let mut inner = self.inner.lock().await;
        if inner.tab.is_none() {
            return Err("no page is open; use browser_open first".into());
        }
        let (cdp, session) = self.current(&mut inner).await?;
        let restore = Restore {
            cdp: Arc::clone(&cdp),
            session: session.clone(),
        };
        let mut runs = Vec::new();
        for scheme in SCHEMES {
            runs.push(run_scheme(&cdp, &session, scheme).await?);
        }
        drop(restore);
        let tab = inner.tab.as_mut().ok_or("no page is open")?;
        let (document, url, log) = {
            let state = tab.state.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.document,
                state.url.clone(),
                page_log(&state, show_secrets),
            )
        };
        tab.forget_refs_of_old_page(document);
        let mut lines = Vec::new();
        for (report, nodes) in runs {
            lines.push(lines_of(report, &nodes, &mut tab.refs, show_secrets)?);
        }
        let dark = lines.pop().unwrap_or_default();
        let light = lines.pop().unwrap_or_default();
        let mut result = merge(light, dark, log);
        result["url"] = json!(short_url(&url));
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn report(items: Value) -> Value {
        json!({"checked": 40, "page": [], "dropped": [], "items": items, "counts": {"contrast": 1}})
    }

    #[test]
    fn element_lines_carry_refs_and_problems_in_both_schemes_are_listed_once() {
        let mut refs = RefMap::default();
        refs.assign(7);
        let item = |text: &str| json!({"contrast": [{"text": text, "describe": "button \"Add task\"", "el": 0}]});
        let light = lines_of(report(item("invisible text")), &[Some(7)], &mut refs, false).unwrap();
        assert_eq!(light.elements, ["invisible text: button \"Add task\" [e1]"]);
        let dark = lines_of(report(item("invisible text")), &[Some(9)], &mut refs, false).unwrap();
        assert_eq!(dark.elements, ["invisible text: button \"Add task\" [e2]"]);
        let same = lines_of(report(item("invisible text")), &[Some(7)], &mut refs, false).unwrap();
        let merged = merge(light, same, vec!["console error: boom".into()]);
        assert_eq!(merged["problems"], 2, "{merged}");
        assert_eq!(
            merged["both"][0],
            "invisible text: button \"Add task\" [e1]"
        );
        assert!(merged.get("light_only").is_none(), "{merged}");
        assert_eq!(merged["page"][0], "console error: boom");
    }

    #[test]
    fn page_problems_of_one_scheme_say_which_and_long_lists_are_cut() {
        let light = SchemeLines {
            page: vec!["quirks".into(), "sideways".into()],
            elements: (0..40).map(|i| format!("problem {i}")).collect(),
            unlisted: 3,
            checked: 10,
            capped: true,
        };
        let dark = SchemeLines {
            page: vec!["quirks".into()],
            ..SchemeLines::default()
        };
        let merged = merge(light, dark, Vec::new());
        assert_eq!(merged["page"], json!(["quirks", "light mode: sideways"]));
        assert_eq!(merged["light_only"].as_array().unwrap().len(), LISTED);
        assert_eq!(merged["problems"], 2 + 40 + 3);
        assert_eq!(merged["not_shown"], 25 + 3);
        assert_eq!(
            merged["checked"],
            "the first 10 elements, in light and dark"
        );
    }

    #[test]
    fn page_strings_lose_url_queries_and_hide_tokens_unless_secrets_are_shown() {
        let line = "failed: http://a.test/x.css?token=abc#f and http://b.test/y.css#frag next";
        assert_eq!(
            shown(line, true),
            "failed: http://a.test/x.css and http://b.test/y.css next"
        );
        let token = "textbox \"sk9f8e7d6c5b4a39281706f5e4d3c2b1a0zz\"";
        assert!(!shown(token, false).contains("sk9f8e7d6c5b4a39281706f5e4d3c2b1a0zz"));
        assert!(shown(token, true).contains("sk9f8e7d6c5b4a39281706f5e4d3c2b1a0zz"));
        let mut refs = RefMap::default();
        let report = json!({
            "page": ["password=hunter2-secret-x on http://a.test/s.css?k=v"],
            "items": {"contrast": [{"text": "api_key=plain-0454", "describe": "textbox", "el": -1}]},
        });
        let lines = lines_of(report, &[], &mut refs, false).unwrap();
        let all = format!("{lines:?}");
        for secret in ["hunter2-secret-x", "k=v", "plain-0454"] {
            assert!(!all.contains(secret), "{secret} in {all}");
        }
    }

    #[test]
    fn a_clean_page_reports_no_problems() {
        let clean = || SchemeLines {
            checked: 12,
            ..SchemeLines::default()
        };
        let merged = merge(clean(), clean(), Vec::new());
        assert_eq!(
            merged,
            json!({"problems": 0, "checked": "12 elements, in light and dark"})
        );
    }

    proptest! {
        // The report comes from the page's script environment: any JSON is read without
        // panicking, and the result stays bounded.
        #[test]
        fn any_report_reads_without_panicking_and_stays_bounded(
            text in ".{0,400}",
            el in any::<i64>(),
            count in any::<u64>(),
            repeat in 0usize..60,
        ) {
            let mut refs = RefMap::default();
            let items: Vec<Value> = (0..repeat)
                .map(|_| json!({"text": text, "describe": text, "el": el}))
                .collect();
            let report = json!({
                "page": vec![text.clone(); repeat],
                "items": {"contrast": items, "other": [1, 2]},
                "counts": {"contrast": count},
            });
            let nodes = vec![Some(1); 3];
            let light = lines_of(report.clone(), &nodes, &mut refs, false).unwrap();
            let dark = lines_of(report, &nodes, &mut refs, false).unwrap();
            let merged = merge(light, dark, Vec::new());
            prop_assert!(merged.to_string().len() < 40_000);
            let _ = lines_of(json!(text), &nodes, &mut refs, false);
            let _ = lines_of(json!({"items": {"contrast": [text]}}), &nodes, &mut refs, true);
            let _ = shown(&text, false);
        }
    }
}
