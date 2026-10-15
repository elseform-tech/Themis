use super::*;
use proptest::prelude::*;
use std::io::Cursor;

struct Fixture {
    _tmp: tempfile::TempDir,
    scope: RecallScope,
    evidence: PathBuf,
    attachments: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let evidence = root.join(".themis/context/thread-1");
    let attachments = root.join(".themis/attachments");
    std::fs::create_dir_all(&evidence).unwrap();
    std::fs::create_dir_all(&attachments).unwrap();
    let scope = RecallScope {
        evidence_dir: evidence.clone(),
        attachments: Vec::new(),
        attachment_root: Some(attachments.clone()),
        project_root: Some(root),
    };
    Fixture {
        _tmp: tmp,
        scope,
        evidence,
        attachments,
    }
}

fn user(text: &str, kind: &str) -> String {
    json!({"role":"User","message_type":"Text","content":text,"evidence_kind":kind}).to_string()
}

fn assistant(text: &str, kind: &str) -> String {
    json!({"role":"Assistant","message_type":"Text","content":text,"evidence_kind":kind})
        .to_string()
}

fn tool(name: &str, payload: &Value, kind: &str) -> String {
    json!({"role":"Tool","message_type":{"ToolResult":[{"id":"c1","call_type":"function","function":{"name":name,"arguments":payload.to_string()},"evidence_kind":kind}]},"content":"","evidence_kind":kind}).to_string()
}

fn write_snapshot(fx: &Fixture, version: u64, lines: &[String]) -> PathBuf {
    let path = fx.evidence.join(format!("snapshot-{version}.jsonl"));
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    path
}

fn args(query: &str) -> RecallArgs {
    RecallArgs {
        query: query.into(),
        ..RecallArgs::default()
    }
}

fn run(fx: &Fixture, query: &str) -> Value {
    run_with(fx, args(query))
}

fn run_with(fx: &Fixture, args: RecallArgs) -> Value {
    recall(&fx.scope, &args, &Limits::default()).unwrap()
}

fn hits(result: &Value) -> &Vec<Value> {
    result["hits"].as_array().unwrap()
}

fn range(hit: &Value) -> (usize, usize) {
    let range = hit["char_range"].as_array().unwrap();
    (
        range[0].as_u64().unwrap() as usize,
        range[1].as_u64().unwrap() as usize,
    )
}

#[test]
fn ranks_phrase_above_all_terms_above_some_terms() {
    let fx = fixture();
    write_snapshot(
        &fx,
        100,
        &[
            user(
                "The harbour was quiet. Somewhere a copper kettle sang.",
                "original",
            ),
            user(
                "A kettle of copper hung by the stove in the harbour house.",
                "original",
            ),
            user("Only a kettle appears in this one.", "original"),
        ],
    );
    let result = run(&fx, "copper kettle");
    let hits = hits(&result);
    assert_eq!(hits.len(), 3, "{result}");
    assert_eq!(hits[0]["match"], "phrase");
    assert_eq!(hits[0]["record"], 1);
    assert_eq!(hits[1]["match"], "all_terms");
    assert_eq!(hits[1]["record"], 2);
    assert_eq!(hits[2]["match"], "some_terms");
    assert_eq!(hits[2]["record"], 3);
    assert_eq!(hits[0]["rank"], 1);
}

#[test]
fn rare_terms_outrank_common_ones_when_the_whole_question_is_the_query() {
    let fx = fixture();
    let mut lines = vec![user("Note: Zanzibar keeps the spare keys.", "original")];
    for index in 0..8 {
        lines.push(user(
            &format!("Meeting {index}: the budget office was discussed again."),
            "original",
        ));
    }
    write_snapshot(&fx, 100, &lines);
    // A whole question: common words ("meeting", "budget") must not bury the rare one.
    let result = run(
        &fx,
        "In which meeting about the budget was the Zanzibar office mentioned?",
    );
    let hits = hits(&result);
    assert_eq!(hits[0]["record"], 1, "{result}");
    assert_eq!(hits[0]["match"], "some_terms");
}

#[test]
fn ranking_stays_deterministic_with_weighted_terms() {
    let fx = fixture();
    write_snapshot(
        &fx,
        100,
        &[
            user("alpha beta", "original"),
            user("alpha gamma", "original"),
            user("beta gamma", "original"),
        ],
    );
    let first = run(&fx, "alpha beta gamma");
    for _ in 0..3 {
        assert_eq!(run(&fx, "alpha beta gamma")["hits"], first["hits"]);
    }
}

#[test]
fn identical_text_across_snapshots_is_reported_from_the_newest() {
    let fx = fixture();
    let line = user(
        "Captain Orlov hid the brass compass under the third floorboard.",
        "original",
    );
    write_snapshot(&fx, 100, std::slice::from_ref(&line));
    write_snapshot(&fx, 200, &[user("unrelated", "original"), line.clone()]);
    write_snapshot(
        &fx,
        300,
        &[user("another", "original"), user("more", "original"), line],
    );
    let result = run(&fx, "brass compass floorboard");
    let hits = hits(&result);
    assert_eq!(hits.len(), 1, "{result}");
    assert_eq!(hits[0]["snapshot"], "snapshot-300.jsonl");
    assert_eq!(hits[0]["record"], 3);
}

#[test]
fn coordinates_locate_the_excerpt_and_hash_the_record() {
    let fx = fixture();
    let text = format!(
        "{}The lighthouse keeper’s name was Marguerite Okonkwo-Lindqvist. {}",
        "é".repeat(900),
        "日本語".repeat(300)
    );
    let line = user(&text, "original");
    write_snapshot(&fx, 100, std::slice::from_ref(&line));
    let result = run(&fx, "Marguerite Okonkwo-Lindqvist");
    let hit = &hits(&result)[0];
    let (start, end) = range(hit);
    let excerpt = hit["excerpt"].as_str().unwrap();
    assert!(excerpt.chars().count() <= EXCERPT_CHARS);
    assert_eq!(excerpt, extract(&text, start, end));
    assert!(excerpt.contains("Marguerite"));
    assert_eq!(hit["pointer"], "/content");
    assert_eq!(hit["role"], "User");
    assert_eq!(hit["evidence_kind"], "original");
    assert_eq!(hit["decoded"], false);
    assert_eq!(hit["sha256"], sha256_hex(line.as_bytes()));
    assert_eq!(hit["path"], ".themis/context/thread-1/snapshot-100.jsonl");
}

#[test]
fn unicode_text_never_splits_characters() {
    let fx = fixture();
    let text = format!(
        "{}火焰之剑 🔥 the ember sword 🔥{}",
        "🌋".repeat(700),
        "ñ".repeat(700)
    );
    write_snapshot(&fx, 100, &[user(&text, "original")]);
    for query in ["ember sword", "火焰之剑"] {
        let result = run(&fx, query);
        let hit = &hits(&result)[0];
        let excerpt = hit["excerpt"].as_str().unwrap();
        assert!(excerpt.chars().count() <= EXCERPT_CHARS);
        assert!(!excerpt.is_empty());
        let (start, end) = range(hit);
        assert_eq!(excerpt, extract(&text, start, end));
    }
}

#[test]
fn matching_ignores_case_and_diacritics() {
    let fx = fixture();
    write_snapshot(
        &fx,
        100,
        &[user(
            "Zoë met Ångström at the Café Münster near São Paulo.",
            "original",
        )],
    );
    for query in ["zoe angstrom", "CAFE MUNSTER", "sao paulo", "café"] {
        let result = run(&fx, query);
        assert_eq!(hits(&result).len(), 1, "{query}: {result}");
    }
    let result = run(&fx, "cafe munster");
    assert_eq!(hits(&result)[0]["match"], "phrase");
}

#[test]
fn tool_results_are_decoded_once_with_coordinates() {
    let fx = fixture();
    let payload = json!({"stdout":"line one\nthe vault code is 7-4-1-9\nline three","exit_code":0});
    write_snapshot(
        &fx,
        100,
        &[
            user("run it", "original"),
            tool("shell", &payload, "original"),
        ],
    );
    let result = run(&fx, "vault code");
    let hit = &hits(&result)[0];
    assert_eq!(hit["decoded"], true);
    assert_eq!(hit["tool"], "shell");
    assert_eq!(
        hit["pointer"],
        "/message_type/ToolResult/0/function/arguments"
    );
    let excerpt = hit["excerpt"].as_str().unwrap();
    assert!(
        excerpt.contains("the vault code is 7-4-1-9\nline three"),
        "{excerpt}"
    );
    assert!(!excerpt.contains("\\n"));
    let filtered = run_with(
        &fx,
        RecallArgs {
            source: "shell".into(),
            ..args("vault code")
        },
    );
    assert_eq!(hits(&filtered).len(), 1);
    let none = run_with(
        &fx,
        RecallArgs {
            source: "git".into(),
            ..args("vault code")
        },
    );
    assert!(hits(&none).is_empty());
}

#[test]
fn recovery_and_navigation_need_explicit_kinds() {
    let fx = fixture();
    write_snapshot(
        &fx,
        100,
        &[
            user("the saffron ledger lives in the cellar", "original"),
            tool(
                "read_file",
                &json!({"content":"saffron ledger copy from recovery"}),
                "recovery",
            ),
            assistant("Checkpoint says the saffron ledger was found", "navigation"),
            assistant("I suggest we hide the saffron ledger", "original_exchange"),
        ],
    );
    let default = run(&fx, "saffron ledger");
    let records: Vec<u64> = hits(&default)
        .iter()
        .map(|h| h["record"].as_u64().unwrap())
        .collect();
    assert_eq!(records.len(), 2, "{default}");
    assert!(records.contains(&1) && records.contains(&4), "{records:?}");
    for kind in ["recovery", "navigation"] {
        let result = run_with(
            &fx,
            RecallArgs {
                kinds: vec![kind.into()],
                ..args("saffron ledger")
            },
        );
        assert_eq!(hits(&result).len(), 1, "{kind}: {result}");
        assert_eq!(hits(&result)[0]["evidence_kind"], kind);
    }
    let error = recall(
        &fx.scope,
        &RecallArgs {
            kinds: vec!["bogus".into()],
            ..args("x")
        },
        &Limits::default(),
    )
    .unwrap_err();
    assert!(error.contains("unknown kind"), "{error}");
}

#[test]
fn mixed_batches_label_each_result() {
    let fx = fixture();
    let record = json!({"role":"Tool","message_type":{"ToolResult":[
        {"id":"a","call_type":"function","function":{"name":"shell","arguments":"{\"stdout\":\"opal tower original\"}"},"evidence_kind":"original"},
        {"id":"b","call_type":"function","function":{"name":"read_file","arguments":"{\"content\":\"opal tower recovery\"}"},"evidence_kind":"recovery"}
    ]},"content":"","evidence_kind":"mixed"})
    .to_string();
    write_snapshot(&fx, 100, &[record]);
    let result = run(&fx, "opal tower");
    assert_eq!(hits(&result).len(), 1);
    assert_eq!(
        hits(&result)[0]["pointer"],
        "/message_type/ToolResult/0/function/arguments"
    );
}

#[test]
fn only_confined_regular_files_are_searched() {
    let fx = fixture();
    write_snapshot(
        &fx,
        100,
        &[user("the mooring fee is nine crowns", "original")],
    );
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(
        outside.path().join("snapshot-999.jsonl"),
        user("mooring fee outside secret", "original") + "\n",
    )
    .unwrap();
    std::fs::write(
        fx.evidence.join("notes.jsonl"),
        user("mooring fee in notes", "original") + "\n",
    )
    .unwrap();
    std::fs::write(
        fx.evidence.join("snapshot-5.jsonl.partial"),
        user("mooring fee partial", "original") + "\n",
    )
    .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        outside.path().join("snapshot-999.jsonl"),
        fx.evidence.join("snapshot-998.jsonl"),
    )
    .unwrap();
    let result = run(&fx, "mooring fee");
    let texts: Vec<&str> = hits(&result)
        .iter()
        .map(|h| h["excerpt"].as_str().unwrap())
        .collect();
    assert_eq!(texts.len(), 1, "{result}");
    assert!(!texts[0].contains("secret") && !texts[0].contains("notes"));
}

#[test]
fn attachments_must_live_in_the_attachment_directory() {
    let mut fx = fixture();
    let folder = fx.attachments.join("id-1");
    std::fs::create_dir_all(&folder).unwrap();
    let inside = folder.join("story.txt");
    std::fs::write(
        &inside,
        "Chapter 9. The beekeeper Wilhelmina counted 41 hives.\n",
    )
    .unwrap();
    let outside = tempfile::tempdir().unwrap();
    let escaped = outside.path().join("secret.txt");
    std::fs::write(&escaped, "The beekeeper confessed everything outside.\n").unwrap();
    fx.scope.attachments = vec![inside.clone(), escaped.clone()];
    #[cfg(unix)]
    {
        let link = folder.join("link.txt");
        std::os::unix::fs::symlink(&escaped, &link).unwrap();
        fx.scope.attachments.push(link);
    }
    let result = run(&fx, "beekeeper hives");
    let found = hits(&result);
    assert_eq!(found.len(), 1, "{result}");
    assert_eq!(found[0]["kind"], "attachment");
    assert_eq!(found[0]["source"], "story.txt");
    assert!(found[0]["record"].is_null());
    assert_eq!(found[0]["pointer"], "");
    let bytes = std::fs::read(&inside).unwrap();
    assert_eq!(found[0]["sha256"], sha256_hex(&bytes));
    let by_source = run_with(
        &fx,
        RecallArgs {
            source: "story".into(),
            ..args("beekeeper")
        },
    );
    assert_eq!(hits(&by_source).len(), 1);
    let other = run_with(
        &fx,
        RecallArgs {
            source: "nothing".into(),
            ..args("beekeeper")
        },
    );
    assert!(hits(&other).is_empty());
    let more = run_with(
        &fx,
        RecallArgs {
            around: found[0]["around"].as_str().unwrap().into(),
            ..RecallArgs::default()
        },
    );
    assert!(more["excerpt"].as_str().unwrap().contains("41 hives"));
    assert_eq!(more["kind"], "attachment");
}

#[test]
fn around_returns_more_context_and_enforces_kinds() {
    let fx = fixture();
    let text = format!(
        "{} NEEDLE-FACT is blue. {}",
        "before ".repeat(500),
        "after ".repeat(500)
    );
    write_snapshot(
        &fx,
        100,
        &[
            user("first", "original"),
            user(&text, "original"),
            tool("read_file", &json!({"content":"recovery blob"}), "recovery"),
        ],
    );
    let result = run(&fx, "needle fact blue");
    let hit = &hits(&result)[0];
    let more = run_with(
        &fx,
        RecallArgs {
            around: hit["around"].as_str().unwrap().into(),
            ..RecallArgs::default()
        },
    );
    let excerpt = more["excerpt"].as_str().unwrap();
    assert!(excerpt.contains("NEEDLE-FACT is blue"));
    assert!(excerpt.chars().count() <= AROUND_CHARS);
    assert!(excerpt.chars().count() > EXCERPT_CHARS);
    assert_eq!(more["sha256"], hit["sha256"]);
    assert!(more["before"].is_string() && more["after"].is_string());
    let again = run_with(
        &fx,
        RecallArgs {
            around: more["after"].as_str().unwrap().into(),
            ..RecallArgs::default()
        },
    );
    assert!(again["char_range"][0].as_u64().unwrap() > more["char_range"][0].as_u64().unwrap());

    let recovery = format!(
        "{}|3|/message_type/ToolResult/0/function/arguments|0",
        hit["path"].as_str().unwrap()
    );
    let error = recall(
        &fx.scope,
        &RecallArgs {
            around: recovery,
            ..RecallArgs::default()
        },
        &Limits::default(),
    )
    .unwrap_err();
    assert!(error.contains("evidence_kind recovery"), "{error}");
    for bad in [
        "nonsense",
        "x|1|/content|0",
        "../../etc/passwd|1|/content|0",
    ] {
        let result = recall(
            &fx.scope,
            &RecallArgs {
                around: bad.into(),
                ..RecallArgs::default()
            },
            &Limits::default(),
        );
        assert!(result.is_err(), "{bad}");
    }
}

#[test]
fn scan_budgets_report_truncation_and_oversized_records_are_skipped() {
    let fx = fixture();
    let big = user(&"filler ".repeat(40_000), "original");
    write_snapshot(
        &fx,
        100,
        &[user("the amber door is locked", "original"), big],
    );
    let tight = Limits {
        scan_bytes: 1 << 40,
        deadline: Duration::from_secs(5),
        record_bytes: 10_000,
    };
    let result = recall(&fx.scope, &args("amber door"), &tight).unwrap();
    assert_eq!(hits(&result).len(), 1);
    assert_eq!(result["truncated_scan"], true);
    assert!(result["truncated_reasons"]
        .as_array()
        .unwrap()
        .contains(&json!("oversized_record")));

    let bytes = Limits {
        scan_bytes: 100,
        deadline: Duration::from_secs(5),
        record_bytes: 1 << 20,
    };
    let result = recall(&fx.scope, &args("amber door"), &bytes).unwrap();
    assert_eq!(result["truncated_scan"], true);

    let none = Limits {
        deadline: Duration::ZERO,
        ..Limits::default()
    };
    let result = recall(&fx.scope, &args("amber"), &none).unwrap();
    assert_eq!(result["truncated_scan"], true);
    assert!(hits(&result).is_empty());

    let complete = run(&fx, "amber door");
    assert_eq!(complete["truncated_scan"], false);
}

#[test]
fn phrase_across_a_chunk_boundary_is_found_once() {
    let fx = fixture();
    let cut = CHUNK_CHARS - CHUNK_OVERLAP;
    let prefix = "x ".repeat(cut / 2 - 3);
    let text = format!(
        "{prefix}the sapphire clock struck thirteen {}",
        "y ".repeat(CHUNK_CHARS / 2)
    );
    write_snapshot(&fx, 100, &[user(&text, "original")]);
    let result = run(&fx, "sapphire clock struck thirteen");
    assert_eq!(hits(&result).len(), 1, "{result}");
    assert_eq!(hits(&result)[0]["match"], "phrase");
    let scan = scan_text(&text, &Query::new("sapphire clock struck thirteen"), None);
    assert_eq!(scan.phrases.len(), 1);
    assert_eq!(scan.occurrences.len(), 4);
    let (start, end) = scan.phrases[0];
    assert!(start >= cut - 40 && start <= cut + 40, "{start} vs {cut}");
    assert_eq!(extract(&text, start, end), "sapphire clock struck thirteen");
}

#[test]
fn limit_defaults_and_is_capped() {
    let fx = fixture();
    let lines: Vec<String> = (0..30)
        .map(|n| {
            user(
                &format!("record {n} mentions the cobalt anchor distinctively {n}"),
                "original",
            )
        })
        .collect();
    write_snapshot(&fx, 100, &lines);
    assert_eq!(hits(&run(&fx, "cobalt anchor")).len(), DEFAULT_LIMIT);
    let many = run_with(
        &fx,
        RecallArgs {
            limit: 99,
            ..args("cobalt anchor")
        },
    );
    assert_eq!(hits(&many).len(), MAX_LIMIT);
    assert_eq!(many["total_matches"], 30);
    assert!(many.to_string().len() <= RESULT_BYTES + 1024);
}

#[test]
fn results_are_deterministic_and_empty_queries_fail() {
    let fx = fixture();
    write_snapshot(
        &fx,
        100,
        &[
            user("alpha beta gamma", "original"),
            user("alpha beta", "original"),
        ],
    );
    assert_eq!(run(&fx, "alpha beta"), run(&fx, "alpha beta"));
    for query in ["", "   ", "!!!"] {
        assert!(recall(&fx.scope, &args(query), &Limits::default()).is_err());
    }
}

#[test]
fn history_source_exports_are_searched() {
    let fx = fixture();
    let path = fx.evidence.join("history-source-1-4-3f2a.jsonl");
    std::fs::write(
        &path,
        json!({"source_seq":2,"role":"User","message_type":"Text","content":"Earlier I asked about the zinc roof.","evidence_kind":"original_exchange"}).to_string() + "\n",
    )
    .unwrap();
    let result = run(&fx, "zinc roof");
    assert_eq!(hits(&result)[0]["kind"], "history_source");
    assert!(hits(&result)[0]["snapshot"].is_null());
}

#[test]
fn query_folding_handles_stopwords_and_prefixes() {
    let query = Query::new("What was the lantern?");
    assert_eq!(query.terms.len(), 1);
    assert_eq!(query.terms[0], vec!['l', 'a', 'n', 't', 'e', 'r', 'n']);
    let all_stop = Query::new("the of");
    assert_eq!(all_stop.terms.len(), 2);
    let scan = scan_text("Three lanterns hung; one Lantern fell.", &query, None);
    assert_eq!(scan.occurrences.len(), 2);
}

proptest! {
    #[test]
    fn record_fields_never_panics_and_is_well_formed(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
        for field in record_fields(&bytes) {
            prop_assert!(field.pointer.starts_with('/'));
        }
    }

    #[test]
    fn record_fields_finds_content_for_any_text(text in "\\PC{1,200}", role in "(User|Assistant|Tool)") {
        let line = json!({"role":role,"message_type":"Text","content":text.clone()}).to_string();
        let fields = record_fields(line.as_bytes());
        prop_assert_eq!(fields.len(), 1);
        prop_assert_eq!(&fields[0].text, &text);
        prop_assert_eq!(&fields[0].pointer, "/content");
    }

    #[test]
    fn tool_payload_decoding_is_single_pass(value in "\\PC{0,80}") {
        // A payload whose string value looks like JSON stays text: decoded once.
        let inner = json!({"a": value.clone()}).to_string();
        let outer = json!({"stdout": inner.clone()}).to_string();
        prop_assert_eq!(decode_envelope(&outer), format!("/stdout:\n{inner}\n"));
        prop_assert_eq!(decode_envelope("plain: text"), "plain: text");
    }

    #[test]
    fn capped_lines_reassemble_and_stay_bounded(
        input in proptest::collection::vec(prop_oneof![Just(b'\n'), Just(b'a'), Just(b'\r'), any::<u8>()], 0..300),
        cap in 0usize..40,
        chunk in 1usize..16,
    ) {
        let mut reader = BufReader::with_capacity(chunk, Cursor::new(input.clone()));
        let mut buffer = Vec::new();
        let mut consumed = 0u64;
        let mut lines = Vec::new();
        loop {
            match read_line_capped(&mut reader, &mut buffer, cap, &mut consumed, &ReadLimit::NONE).unwrap() {
                Line::End | Line::Exhausted => break,
                Line::Complete => lines.push(Some(buffer.clone())),
                Line::Oversize => lines.push(None),
            }
            prop_assert!(buffer.len() <= cap);
        }
        prop_assert_eq!(consumed, input.len() as u64);
        let mut expected: Vec<&[u8]> = input.split(|b| *b == b'\n').collect();
        if expected.last().is_some_and(|p| p.is_empty()) {
            expected.pop();
        }
        prop_assert_eq!(lines.len(), expected.len());
        for (got, want) in lines.iter().zip(expected) {
            match got {
                Some(line) => {
                    prop_assert!(want.len() <= cap);
                    let trimmed = want.strip_suffix(b"\r").unwrap_or(want);
                    prop_assert_eq!(line.as_slice(), trimmed);
                }
                None => prop_assert!(want.len() > cap),
            }
        }
    }

    #[test]
    fn scanning_arbitrary_text_yields_in_bounds_ranges(text in "\\PC{0,600}", query in "[a-zA-Zé ]{1,20}") {
        let q = Query::new(&query);
        prop_assume!(!q.terms.is_empty());
        let total = text.chars().count();
        let scan = scan_text(&text, &q, None);
        for o in &scan.occurrences {
            prop_assert!(o.start < o.end && o.end <= total);
        }
        for window in best_windows(&scan, q.terms.len()) {
            prop_assert!(window.start < window.end && window.end <= total);
            prop_assert!((1..=3).contains(&window.tier));
            let _ = extract(&text, window.start, window.end);
        }
    }

    #[test]
    fn folding_is_idempotent(text in "\\PC{0,100}") {
        let once: String = text.chars().filter_map(fold_char).collect();
        let twice: String = once.chars().filter_map(fold_char).collect();
        prop_assert_eq!(once, twice);
    }
}

#[test]
fn placeholder_around_values_do_not_block_a_search() {
    let fx = fixture();
    write_snapshot(
        &fx,
        100,
        &[user("the tin whistle is in the drawer", "original")],
    );
    for placeholder in ["0", "none", "null", " "] {
        let result = run_with(
            &fx,
            RecallArgs {
                around: placeholder.into(),
                ..args("tin whistle")
            },
        );
        assert_eq!(hits(&result).len(), 1, "{placeholder}");
    }
    // Without a query the value is still treated as an (invalid) expansion.
    let error = recall(
        &fx.scope,
        &RecallArgs {
            around: "0".into(),
            ..RecallArgs::default()
        },
        &Limits::default(),
    );
    assert!(error.is_err());
}

#[test]
fn the_byte_budget_is_enforced_inside_a_single_huge_line() {
    // One 5 MB line with no newline: the reader stops at the budget instead of
    // consuming the line, and never holds more than the cap.
    let huge = vec![b'a'; 5 * 1024 * 1024];
    let mut reader = BufReader::with_capacity(8192, Cursor::new(huge.clone()));
    let mut buffer = Vec::new();
    let mut consumed = 0u64;
    let limit = ReadLimit {
        max_bytes: 100_000,
        deadline: None,
    };
    let outcome =
        read_line_capped(&mut reader, &mut buffer, 1 << 20, &mut consumed, &limit).unwrap();
    assert_eq!(outcome, Line::Exhausted);
    assert_eq!(consumed, 100_000);
    assert!(buffer.is_empty());
    // A whole-slice reader (one giant fill_buf) is clamped too.
    let mut cursor = Cursor::new(huge);
    let mut consumed = 0u64;
    let outcome =
        read_line_capped(&mut cursor, &mut buffer, 1 << 20, &mut consumed, &limit).unwrap();
    assert_eq!(outcome, Line::Exhausted);
    assert_eq!(consumed, 100_000);
    // An expired deadline stops before any byte is read.
    let expired = ReadLimit {
        max_bytes: u64::MAX,
        deadline: Some(Instant::now()),
    };
    let mut consumed = 0u64;
    let mut cursor = Cursor::new(b"abc\n".to_vec());
    let outcome = read_line_capped(&mut cursor, &mut buffer, 16, &mut consumed, &expired).unwrap();
    assert_eq!((outcome, consumed), (Line::Exhausted, 0));
}

#[test]
fn match_caps_are_reported_in_truncated_scan() {
    let fx = fixture();
    // More than MAX_OCCURRENCES matches of one term in a single record.
    let text = "ember ".repeat(MAX_OCCURRENCES + 500);
    write_snapshot(&fx, 100, &[user(&text, "original")]);
    let result = run(&fx, "ember");
    assert_eq!(result["truncated_scan"], true);
    assert!(result["truncated_reasons"]
        .as_array()
        .unwrap()
        .contains(&json!("occurrence_cap")));
    assert_eq!(hits(&result).len(), 1, "identical windows dedupe");
    let scan = scan_text(&text, &Query::new("ember"), None);
    assert!(scan.capped);
    assert!(scan.phrases.len() <= MAX_OCCURRENCES);
    assert!(scan.occurrences.len() <= MAX_OCCURRENCES);
}

proptest! {
    #[test]
    fn byte_budget_bounds_consumption_for_any_input(
        input in proptest::collection::vec(any::<u8>(), 0..400),
        budget in 0u64..450,
        cap in 0usize..64,
        chunk in 1usize..32,
    ) {
        let mut reader = BufReader::with_capacity(chunk, Cursor::new(input.clone()));
        let mut buffer = Vec::new();
        let mut consumed = 0u64;
        let limit = ReadLimit { max_bytes: budget, deadline: None };
        loop {
            match read_line_capped(&mut reader, &mut buffer, cap, &mut consumed, &limit).unwrap() {
                Line::End | Line::Exhausted => break,
                _ => {}
            }
            prop_assert!(consumed <= budget);
            prop_assert!(buffer.len() <= cap);
        }
        prop_assert!(consumed <= budget);
        prop_assert!(consumed <= input.len() as u64);
    }
}

#[cfg(unix)]
#[test]
fn symlinked_sources_are_not_opened() {
    let fx = fixture();
    let real = fx.evidence.join("real.txt");
    std::fs::write(&real, "{}").unwrap();
    let link = fx.evidence.join("snapshot-7.jsonl");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(open_regular(&link).is_none());
    assert!(open_regular(&real).is_some());
    assert!(open_regular(&fx.evidence).is_none());
}

#[test]
fn around_stays_under_the_delivery_budget_and_pages_contiguously() {
    let fx = fixture();
    // Control characters escape to six bytes each in JSON.
    let text = "\u{1}".repeat(5000) + &"needle ".repeat(10) + &"\u{2}".repeat(5000);
    write_snapshot(&fx, 100, &[user(&text, "original")]);
    let result = run(&fx, "needle");
    let token = hits(&result)[0]["around"].as_str().unwrap().to_owned();
    let page = run_with(
        &fx,
        RecallArgs {
            around: token,
            ..RecallArgs::default()
        },
    );
    assert!(
        page.to_string().len() <= RESULT_BYTES,
        "{}",
        page.to_string().len()
    );
    let (s, e) = range(&page);
    assert!(e > s);
    let next = run_with(
        &fx,
        RecallArgs {
            around: page["after"].as_str().unwrap().into(),
            ..RecallArgs::default()
        },
    );
    assert_eq!(
        range(&next).0,
        e,
        "the next page starts where this one ended"
    );
}

#[test]
fn identical_text_from_different_roles_is_not_deduplicated() {
    let fx = fixture();
    write_snapshot(
        &fx,
        100,
        &[
            user("the copper bell rings at noon", "original"),
            assistant("the copper bell rings at noon", "original_exchange"),
        ],
    );
    let result = run(&fx, "copper bell noon");
    assert_eq!(hits(&result).len(), 2, "{result}");
}

proptest! {
    #[test]
    fn term_weights_are_positive_and_favour_rarer_terms(
        fields in 0u64..1_000_000,
        mut found in proptest::collection::vec(0u64..1_000_000, 1..16),
    ) {
        for count in &mut found {
            *count = (*count).min(fields);
        }
        let weights = term_weights(fields, &found);
        prop_assert_eq!(weights.len(), found.len());
        for (a, wa) in found.iter().zip(&weights) {
            prop_assert!(*wa > 0);
            for (b, wb) in found.iter().zip(&weights) {
                if a < b {
                    prop_assert!(wa >= wb);
                }
            }
        }
    }
}
