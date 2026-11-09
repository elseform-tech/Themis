/// The reference is valid, changes nothing (its values are the defaults) and names every
/// field, so a new setting cannot be added without documenting it there.
#[test]
fn set_top_level_keeps_comments_of_a_file_without_settings() {
    let content = "// header\n{\n  // \"sandbox\": { \"network\": \"block\" },\n}\n";
    let updated =
        set_top_level(content, "browser", &serde_json::json!({ "enabled": false })).unwrap();
    assert!(updated.contains("// \"sandbox\": { \"network\": \"block\" },"));
    assert_eq!(
        parse(&updated).unwrap()["browser"]["enabled"],
        serde_json::json!(false)
    );
    let bare = set_top_level("{}", "web", &serde_json::json!({})).unwrap();
    assert_eq!(parse(&bare).unwrap()["web"], serde_json::json!({}));
}
#[test]
fn the_reference_is_valid_and_lists_every_setting() {
    let resolved = resolve(Some(REFERENCE), None, None).expect("reference resolves");
    let reference = serde_json::to_value(&resolved.config).unwrap();
    let defaults = serde_json::to_value(RuntimeConfig::default()).unwrap();
    assert_eq!(reference, defaults, "the reference must show the defaults");
    // Optional fields shown commented out are named in the text.
    let text = REFERENCE;
    // Every field the configuration has (from its defaults), plus the optional ones that
    // have no default value, must be named in the reference.
    fn fields(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::Object(object) => {
                for (name, inner) in object {
                    out.push(name.clone());
                    fields(inner, out);
                }
            }
            Value::Array(items) => items.iter().for_each(|item| fields(item, out)),
            _ => {}
        }
    }
    let mut names = vec![
        "executable",
        "downloads_directory",
        "search_url",
        "context_token_budget",
        "recent_messages",
        "total_turns",
        "summary_model",
        "model",
        "prompt_file",
        "command",
        "downloads",
        "debugging",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    fields(&defaults, &mut names);
    for name in names
        .iter()
        .filter(|name| name.as_str() != "site_restrictions")
    {
        assert!(
            text.contains(&format!("\"{name}\"")),
            "the reference does not mention \"{name}\""
        );
    }
    // The schema editors use knows the same fields.
    let schema = schema().to_string();
    for name in &names {
        assert!(
            schema.contains(&format!("\"{name}\"")),
            "the schema does not know \"{name}\""
        );
    }
}

use super::*;

#[test]
fn web_and_browser_settings_come_from_the_user_file_and_projects_may_only_disable() {
    // An absolute path on this OS.
    let chrome = if cfg!(windows) {
        r"C:\\Chrome\\chrome.exe"
    } else {
        "/opt/chrome"
    };
    let user = format!(
        r#"{{"web":{{"search_url":"http://127.0.0.1:8888"}},"browser":{{"executable":"{chrome}"}}}}"#
    );
    let user = user.as_str();
    let resolved = resolve(Some(user), Some(r#"{"browser":{"enabled":false}}"#), None).unwrap();
    assert_eq!(
        resolved.config.web.search_url.as_deref(),
        Some("http://127.0.0.1:8888")
    );
    assert!(!resolved.config.browser.enabled);
    assert_eq!(resolved.provenance["browser.enabled"], "project");
    assert!(RuntimeConfig::default().browser.enabled);
    for project in [
        r#"{"web":{"search_url":"https://evil.example"}}"#,
        r#"{"browser":{"executable":"/tmp/x"}}"#,
        r#"{"browser":{"enabled":true}}"#,
    ] {
        assert!(
            resolve(Some(user), Some(project), None).is_err(),
            "{project}"
        );
        assert!(
            resolve(None, None, Some(project)).is_err(),
            "run: {project}"
        );
    }
    for bad in [
        r#"{"web":{"search_url":"ftp://x"}}"#,
        r#"{"web":{"search_url":"http://u:p@x"}}"#,
        r#"{"web":{"search_url":"not a url"}}"#,
        r#"{"browser":{"executable":"chrome"}}"#,
        r#"{"web":{"other":1}}"#,
    ] {
        assert!(resolve(Some(bad), None, None).is_err(), "{bad}");
    }
    assert!(schema()["properties"]["web"]["properties"]["search_url"].is_object());
}

#[test]
fn site_rules_match_the_host_and_its_subdomains() {
    let action = |tool: &str, host: &str| ToolAction {
        display: None,
        command: vec![host.into()],
        tool: tool.into(),
        summary: String::new(),
        risk: RiskLevel::Network,
    };
    // A browser approval's command is the site, then the action.
    let browser = |what: &str, host: &str| ToolAction {
        command: vec![host.into(), what.into()],
        ..action("browser", host)
    };
    let resolved = resolve(
            Some(r#"{"approval":{"rules":[{"tool":"fetch_url","command":["docs.rs"],"action":"allow"}]}}"#),
            Some(r#"{"approval":{"rules":[{"tool":"browser","command":["Example.com"],"action":"deny"}]}}"#),
            None,
        )
        .unwrap();
    let policy = &resolved.config.approval;
    assert_eq!(
        policy.decision(&browser("click", "example.com")),
        PolicyAction::Deny
    );
    assert_eq!(
        policy.decision(&browser("open", "login.example.com")),
        PolicyAction::Deny
    );
    assert_eq!(
        policy.decision(&browser("open", "notexample.com")),
        PolicyAction::Ask
    );
    assert_eq!(
        policy.decision(&action("fetch_url", "docs.rs")),
        PolicyAction::Allow
    );
    assert_eq!(
        policy.decision(&action("fetch_url", "evil-docs.rs")),
        PolicyAction::Ask
    );
    assert!(site_matches("A.Example.COM.", "example.com"));
    assert!(!site_matches("example.com", ""));
    for bad in [
        r#"{"tool":"fetch_url","command":["https://docs.rs"],"action":"deny"}"#,
        r#"{"tool":"fetch_url","command":["a.com","b.com"],"action":"deny"}"#,
        r#"{"tool":"fetch_url","command":["*.a.com"],"action":"deny"}"#,
        r#"{"tool":"read_file","command":["a.com"],"action":"deny"}"#,
        r#"{"tool":"browser","command":["a.com","b.com"],"action":"deny"}"#,
        r#"{"tool":"browser","command":["click","open"],"action":"deny"}"#,
        r#"{"tool":"browser","command":["a.com","click","x.com"],"action":"deny"}"#,
        r#"{"tool":"browser","command":["*.a.com"],"action":"deny"}"#,
        r#"{"tool":"browser","command":["https://a.com","click"],"action":"deny"}"#,
    ] {
        let file = format!(r#"{{"approval":{{"rules":[{bad}]}}}}"#);
        assert!(resolve(Some(&file), None, None).is_err(), "{bad}");
    }
}

/// A browser rule names a site, an action (or `download`), or both, in either order; `browser`
/// covers every action, and a rule naming an action covers only that one.
#[test]
fn browser_rules_name_a_site_an_action_or_both() {
    let browser = |what: &str, host: &str| ToolAction {
        display: None,
        command: vec![host.into(), what.into()],
        tool: "browser".into(),
        summary: String::new(),
        risk: RiskLevel::Network,
    };
    let resolved = resolve(
        Some(
            r#"{"approval":{"rules":[
                {"tool":"browser","command":["devtools"],"action":"deny"},
                {"tool":"browser","command":["click","shop.example"],"action":"deny"},
                {"tool":"browser","command":["docs.rs","download"],"action":"allow"},
                {"tool":"browser*","command":["localhost"],"action":"allow"},
                {"tool":"browser","command":["open"],"action":"allow"}
            ]}}"#,
        ),
        None,
        None,
    )
    .unwrap();
    let policy = &resolved.config.approval;
    for (what, host, decision) in [
        ("devtools", "example.com", PolicyAction::Deny),
        ("devtools", "localhost", PolicyAction::Deny),
        ("click", "shop.example", PolicyAction::Deny),
        ("click", "a.shop.example", PolicyAction::Deny),
        ("type", "shop.example", PolicyAction::Ask),
        ("download", "docs.rs", PolicyAction::Allow),
        ("download", "example.com", PolicyAction::Ask),
        ("click", "localhost", PolicyAction::Allow),
        ("open", "example.com", PolicyAction::Allow),
        ("navigate", "example.com", PolicyAction::Ask),
    ] {
        assert_eq!(
            policy.decision(&browser(what, host)),
            decision,
            "{what} on {host}"
        );
    }
    // The browser tools of old are gone: a rule naming one is an error, not a rule that
    // quietly matches nothing.
    for old in ["browser_*", "browser_click", "browser_devtools*"] {
        let file = format!(r#"{{"approval":{{"rules":[{{"tool":"{old}","action":"deny"}}]}}}}"#);
        let error = resolve(Some(&file), None, None).unwrap_err().to_string();
        assert!(error.contains("no longer exists"), "{old}: {error}");
    }
}

#[test]
fn set_top_level_replaces_one_value_and_keeps_every_comment() {
    let file = "{\n  // my model\n  \"schema_version\": 1, /* keep */\n  \"browser\": { \"enabled\": true } // old\n}\n";
    let browser = serde_json::json!({"enabled": false, "show_secrets": true});
    let updated = set_top_level(file, "browser", &browser).unwrap();
    assert!(
        updated.contains("// my model")
            && updated.contains("/* keep */")
            && updated.contains("// old"),
        "{updated}"
    );
    assert_eq!(parse(&updated).unwrap()["browser"], browser);
    assert_eq!(parse(&updated).unwrap()["schema_version"], 1);
    // Absent key: appended, with or without other keys, and to an empty file.
    let added = set_top_level("{ \"schema_version\": 1, }", "browser", &browser).unwrap();
    assert_eq!(parse(&added).unwrap()["browser"], browser);
    assert_eq!(parse(&added).unwrap()["schema_version"], 1);
    assert_eq!(
        parse(&set_top_level("{}", "browser", &browser).unwrap()).unwrap()["browser"],
        browser
    );
    assert_eq!(
        parse(&set_top_level("", "browser", &browser).unwrap()).unwrap()["browser"],
        browser
    );
    // A nested key with the same name is not the top-level one.
    let nested = "{\"web\": {\"browser\": 1}}";
    let updated = set_top_level(nested, "browser", &browser).unwrap();
    assert_eq!(parse(&updated).unwrap()["web"]["browser"], 1);
    assert_eq!(parse(&updated).unwrap()["browser"], browser);
    assert!(set_top_level("[1]", "browser", &browser).is_err());
}

#[test]
fn browser_permissions_default_and_projects_may_only_tighten() {
    let resolved = resolve(None, None, None).unwrap().config.browser;
    // Workspace stays closed when the agent browses; the overview box previews it.
    assert!(resolved.developer_mode && !resolved.open_workspace && !resolved.show_secrets);
    // Screenshots reach a model that reads images unless the person turns it off.
    assert!(resolved.screenshots_to_model);
    assert_eq!(resolved.permissions.debugging, SiteAccess::Block);
    assert_eq!(resolved.site_exceptions.len(), STARTER_BLOCKED_SITES.len());
    // The user's list replaces the starter list, so entries can be removed.
    let user = r#"{"browser":{"site_exceptions":[{"site":"example.com","debugging":"allow"}],"permissions":{"debugging":"ask"}}}"#;
    let resolved = resolve(Some(user), None, None).unwrap().config.browser;
    assert_eq!(resolved.site_exceptions.len(), 1);
    assert_eq!(resolved.permissions.debugging, SiteAccess::Ask);
    assert_eq!(
        resolved.permissions.browsing,
        SiteAccess::Allow,
        "unset fields keep their default"
    );
    // A project may block more; its exceptions are kept apart and do not replace the user's.
    let project = r#"{"browser":{"site_exceptions":[{"site":"admin.example.com","browsing":"block"}],"permissions":{"downloads":"block"},"developer_mode":false}}"#;
    let resolved = resolve(Some(user), Some(project), None)
        .unwrap()
        .config
        .browser;
    assert_eq!(resolved.site_exceptions.len(), 1);
    assert_eq!(resolved.site_restrictions[0].site, "admin.example.com");
    assert_eq!(resolved.permissions.downloads, SiteAccess::Block);
    assert!(!resolved.developer_mode);
    let off = r#"{"browser":{"screenshots_to_model":false}}"#;
    assert!(
        !resolve(Some(user), Some(off), None)
            .unwrap()
            .config
            .browser
            .screenshots_to_model
    );
    // It may not loosen anything.
    for loosen in [
        r#"{"browser":{"permissions":{"debugging":"allow"}}}"#,
        r#"{"browser":{"site_exceptions":[{"site":"paypal.com","browsing":"allow"}]}}"#,
        r#"{"browser":{"developer_mode":true}}"#,
        r#"{"browser":{"show_secrets":true}}"#,
        r#"{"browser":{"screenshots_to_model":true}}"#,
        r#"{"browser":{"downloads_directory":"/tmp"}}"#,
        r#"{"browser":{"open_workspace":true}}"#,
        r#"{"browser":{"site_restrictions":[]}}"#,
    ] {
        assert!(resolve(Some(user), Some(loosen), None).is_err(), "{loosen}");
    }
    // Sites are lowercase hosts.
    for bad in ["https://example.com", "Example.com", "example.com/path", ""] {
        let file = format!(
            r#"{{"browser":{{"site_exceptions":[{{"site":"{bad}","browsing":"block"}}]}}}}"#
        );
        assert!(resolve(Some(&file), None, None).is_err(), "{bad}");
    }
    assert!(resolve(
        Some(r#"{"browser":{"downloads_directory":"relative"}}"#),
        None,
        None
    )
    .is_err());
}

#[test]
fn summary_model_override_follows_precedence_and_can_be_cleared() {
    let user = r#"{"compaction":{"summary_model":"gpt-5.6-luna"}}"#;
    let project = r#"{"compaction":{"summary_model":"gpt-6-luna"}}"#;
    let resolved = resolve(Some(user), Some(project), None).unwrap();
    assert_eq!(
        serde_json::to_value(&resolved.config).unwrap()["compaction"]["summary_model"],
        "gpt-6-luna"
    );
    assert_eq!(resolved.provenance["compaction.summary_model"], "project");
    let cleared = resolve(
        Some(user),
        Some(project),
        Some(r#"{"compaction":{"summary_model":null}}"#),
    )
    .unwrap();
    assert!(serde_json::to_value(cleared.config).unwrap()["compaction"]["summary_model"].is_null());
    assert_eq!(cleared.provenance["compaction.summary_model"], "run");
    // Any model id is accepted here; a run checks it against the captured catalogs.
    // A prefix must name a provider from `providers`: a bare id is OpenCode Go.
    let local = r#""providers":[{"id":"local","name":"Local","base_url":"http://127.0.0.1:11434/v1","secret":"${LOCAL_KEY}"}]"#;
    for (model, providers) in [("step-5-preview-free", ""), ("local/qwen/coder", local)] {
        let separator = if providers.is_empty() { "" } else { "," };
        assert!(
            resolve(
                Some(&format!(
                    r#"{{"compaction":{{"summary_model":"{model}"}}{separator}{providers}}}"#
                )),
                None,
                None
            )
            .is_ok(),
            "{model}"
        );
    }
    let unknown = resolve(
        Some(r#"{"compaction":{"summary_model":"local/qwen/coder"}}"#),
        None,
        None,
    )
    .unwrap_err()
    .to_string();
    assert!(unknown.contains("names provider 'local'"), "{unknown}");
    for model in ["", "has space", "line\\nbreak"] {
        assert!(resolve(
            Some(&format!(
                r#"{{"compaction":{{"summary_model":"{model}"}}}}"#
            )),
            None,
            None
        )
        .is_err());
    }
}
#[test]
fn built_in_hooks_and_the_memory_switch_are_gone() {
    // The built-in hooks and the memory switch are gone, not kept as aliases.
    assert!(resolve(
        Some(r#"{"hooks":{"built_in":{"still_open":true}}}"#),
        None,
        None
    )
    .is_err());
    assert!(resolve(Some(r#"{"memory":{"auto":true}}"#), None, None).is_err());
}

#[test]
fn memory_models_default_to_three_cheap_models_and_are_the_persons_own() {
    let defaults = RuntimeConfig::default().memory;
    assert_eq!(
        defaults.models,
        [
            "muse-spark-1.3-contributor",
            "claude-haiku-5-5",
            "deepseek-v4.1-flash"
        ]
    );
    assert!((defaults.daily_budget_usd - 0.5).abs() < f64::EPSILON);
    assert_eq!(defaults.consolidate_every_hours, 24);
    let user = resolve(
            Some(r#"{"memory":{"models":["a","b","c","d"],"daily_budget_usd":2,"consolidate_every_hours":6}}"#),
            None,
            None,
        )
        .unwrap();
    assert_eq!(user.config.memory.models, ["a", "b", "c", "d"]);
    assert_eq!(user.provenance["memory.models"], "user");
    assert_eq!(user.config.memory.consolidate_every_hours, 6);
    // Fewer than three, a duplicate, a bad id, a bad budget or interval: refused.
    for bad in [
        r#"{"memory":{"models":["a","b"]}}"#,
        r#"{"memory":{"models":[]}}"#,
        r#"{"memory":{"models":["a","b","a"]}}"#,
        r#"{"memory":{"models":["a","b","has space"]}}"#,
        r#"{"memory":{"models":["a","b","c","d","e","f","g","h","i"]}}"#,
        r#"{"memory":{"models":["a","b","local/c"]}}"#,
        r#"{"memory":{"daily_budget_usd":-1}}"#,
        r#"{"memory":{"daily_budget_usd":1000}}"#,
        r#"{"memory":{"consolidate_every_hours":0}}"#,
    ] {
        assert!(resolve(Some(bad), None, None).is_err(), "{bad}");
    }
    // A project or a run may not change the models, the cap or the interval.
    for field in [
        r#"{"memory":{"models":["a","b","c"]}}"#,
        r#"{"memory":{"daily_budget_usd":0.1}}"#,
        r#"{"memory":{"consolidate_every_hours":48}}"#,
    ] {
        assert!(resolve(None, Some(field), None).is_err(), "{field}");
        assert!(resolve(None, None, Some(field)).is_err(), "{field}");
    }
}

#[test]
fn memory_is_on_by_default_and_projects_may_only_turn_it_off() {
    assert!(RuntimeConfig::default().memory.enabled);
    let off = resolve(Some(r#"{"memory":{"enabled":false}}"#), None, None).unwrap();
    assert!(!off.config.memory.enabled);
    assert_eq!(off.provenance["memory.enabled"], "user");
    let project = resolve(None, Some(r#"{"memory":{"enabled":false}}"#), None).unwrap();
    assert!(!project.config.memory.enabled);
    assert!(resolve(
        Some(r#"{"memory":{"enabled":false}}"#),
        Some(r#"{"memory":{"enabled":true}}"#),
        None
    )
    .is_err());
    assert!(resolve(None, None, Some(r#"{"memory":{"enabled":true}}"#)).is_err());
    assert!(resolve(Some(r#"{"memory":{"other":1}}"#), None, None).is_err());
}

#[test]
fn subagent_settings_resolve_and_projects_may_only_lower_them() {
    let resolved = resolve(
            Some(r#"{"subagents":{"max_parallel":5,"model":"glm-5.3","models":["glm-5","beam-free"]}}"#),
            Some(r#"{"subagents":{"max_parallel":2,"enabled":false,"allow_writes":false,"allow_mcp":false,"allow_web":false,"models":["glm-5"]}}"#),
            None,
        )
        .unwrap();
    let subagents = &resolved.config.subagents;
    assert_eq!(subagents.max_parallel, 2);
    assert!(!subagents.enabled && !subagents.allow_writes && !subagents.allow_mcp);
    assert!(!subagents.allow_web);
    assert_eq!(resolved.provenance["subagents.allow_mcp"], "project");
    assert_eq!(resolved.provenance["subagents.allow_web"], "project");
    assert_eq!(subagents.max_turns, 40);
    assert_eq!(subagents.model.as_deref(), Some("glm-5.3"));
    assert_eq!(subagents.models, ["glm-5"]);
    assert_eq!(resolved.provenance["subagents.max_parallel"], "project");
    for project in [
        r#"{"subagents":{"max_parallel":4}}"#,
        r#"{"subagents":{"max_turns":41}}"#,
        r#"{"subagents":{"enabled":true}}"#,
        r#"{"subagents":{"allow_writes":true}}"#,
        r#"{"subagents":{"allow_mcp":true}}"#,
        r#"{"subagents":{"allow_web":true}}"#,
        r#"{"subagents":{"allow_mcp":"no"}}"#,
        r#"{"subagents":{"models":["claude-opus"]}}"#,
    ] {
        assert!(resolve(None, Some(project), None).is_err(), "{project}");
    }
    for user in [
        r#"{"subagents":{"max_parallel":0}}"#,
        r#"{"subagents":{"max_parallel":7}}"#,
        r#"{"subagents":{"max_turns":0}}"#,
        r#"{"subagents":{"model":"nope/model"}}"#,
        r#"{"subagents":{"models":["a","b","c","d"]}}"#,
        r#"{"subagents":{"models":["nope/model"]}}"#,
        r#"{"subagents":{"unknown":1}}"#,
        r#"{"hooks":{"max_wakes":11}}"#,
        r#"{"hooks":{"unknown":1}}"#,
    ] {
        assert!(resolve(Some(user), None, None).is_err(), "{user}");
    }
}

#[test]
fn models_keys_need_a_known_provider_prefix() {
    // A `models` key may also name OpenCode Go as go/<id>; another prefix needs a provider.
    assert!(resolve(
        Some(r#"{"models":{"go/glm-5.3":{"context_tokens":64000}}}"#),
        None,
        None
    )
    .is_ok());
    assert!(resolve(
        Some(r#"{"models":{"glm-5.3":{"context_tokens":64000}}}"#),
        None,
        None
    )
    .is_ok());
    let typo = resolve(
        Some(r#"{"models":{"lcoal/qwen":{"context_tokens":64000}}}"#),
        None,
        None,
    )
    .unwrap_err()
    .to_string();
    assert!(typo.contains("names provider 'lcoal'"), "{typo}");
}

#[test]
fn document_batch_and_summary_backoff_have_bounded_independent_configuration() {
    let mut config = RuntimeConfig::default();
    assert_eq!(config.compaction.document_batch_timeout_seconds, 3600);
    assert_eq!(config.compaction.rate_limit_backoff_seconds, 10);
    for invalid in [0, 21601] {
        config.compaction.document_batch_timeout_seconds = invalid;
        assert!(validate(&config)
            .unwrap_err()
            .to_string()
            .contains("document_batch_timeout_seconds"));
    }
    config.compaction.document_batch_timeout_seconds = 21600;
    for invalid in [0, 301] {
        config.compaction.rate_limit_backoff_seconds = invalid;
        assert!(validate(&config)
            .unwrap_err()
            .to_string()
            .contains("rate_limit_backoff_seconds"));
    }
    config.compaction.rate_limit_backoff_seconds = 300;
    validate(&config).unwrap();
    let schema = schema();
    assert_eq!(
        schema["properties"]["compaction"]["properties"]["document_batch_timeout_seconds"]
            ["maximum"],
        21600
    );
    assert_eq!(
        schema["properties"]["compaction"]["properties"]["rate_limit_backoff_seconds"]["default"],
        10
    );
}
#[test]
fn document_input_capacity_is_validated_independently_of_ordinary_context() {
    let mut config = RuntimeConfig {
        context_token_budget: Some(ContextTokenBudget::Tokens(24000)),
        compaction: CompactionConfig {
            document_context_token_budget: 1000000,
            ..Default::default()
        },
        ..Default::default()
    };
    validate(&config).unwrap();
    assert_eq!(
        config.context_token_budget,
        Some(ContextTokenBudget::Tokens(24000))
    );
    config.compaction.document_context_token_budget = 1000001;
    assert!(validate(&config)
        .unwrap_err()
        .to_string()
        .contains("document_context_token_budget"));
    config.compaction.document_context_token_budget = 23999;
    assert!(validate(&config).is_err());
}

#[test]
fn adaptive_context_configuration_accepts_256k_and_validates_shared_controls() {
    let source = r#"{"context_token_budget":256000,"compaction":{"maintenance_threshold_ratio":0.83,"semantic_threshold_ratio":0.93,"target_headroom_interactions":6,"timeout_seconds":600,"request_timeout_seconds":300},"logging":{"context_metrics":"detailed"}}"#;
    let resolved = resolve(Some(source), None, None).unwrap();
    assert_eq!(
        resolved.config.context_token_budget,
        Some(ContextTokenBudget::Tokens(256000))
    );
    assert_eq!(
        resolved.provenance["compaction.request_timeout_seconds"],
        "user"
    );
    assert!(resolve(Some(r#"{"context_token_budget":1000000}"#), None, None).is_ok());
    assert!(resolve(Some(r#"{"context_token_budget":1000001}"#), None, None).is_err());
    assert!(resolve(Some(r#"{"context_token_budget":1999}"#), None, None).is_err());
    assert!(resolve(Some(r#"{"context_token_budget":"big"}"#), None, None).is_err());
    for source in [
        r#"{"compaction":{"maintenance_threshold_ratio":0.96,"semantic_threshold_ratio":0.93}}"#,
        r#"{"compaction":{"semantic_threshold_ratio":1.0}}"#,
        r#"{"compaction":{"target_headroom_interactions":0}}"#,
        r#"{"compaction":{"request_timeout_seconds":0}}"#,
        r#"{"logging":{"context_metrics":"payloads"}}"#,
    ] {
        assert!(resolve(Some(source), None, None).is_err(), "{source}");
    }
}
#[test]
fn longer_compaction_defaults_preserve_explicit_legacy_timeouts_and_match_schema() {
    assert_eq!(
        resolve(None, None, None)
            .unwrap()
            .config
            .compaction
            .timeout_seconds,
        900
    );
    assert!(resolve(
        Some(r#"{"compaction":{"timeout_seconds":300}}"#),
        None,
        None
    )
    .is_ok());
    assert!(resolve(
        Some(r#"{"compaction":{"timeout_seconds":1801}}"#),
        None,
        None
    )
    .is_err());
    assert!(resolve(Some(r#"{"compaction":{"timeout_seconds":0}}"#), None, None).is_err());
    let defaults = resolve(None, None, None).unwrap().config.compaction;
    assert_eq!(defaults.retry_limit, 2);
    assert_eq!(defaults.request_timeout_seconds, 300);
    for limit in 0..=2 {
        assert!(resolve(
            Some(&format!(r#"{{"compaction":{{"retry_limit":{limit}}}}}"#)),
            None,
            None
        )
        .is_ok());
    }
    assert!(resolve(Some(r#"{"compaction":{"retry_limit":3}}"#), None, None).is_err());
    assert_eq!(
        resolve(
            Some(r#"{"compaction":{"timeout_seconds":300,"retry_limit":1}}"#),
            None,
            None
        )
        .unwrap()
        .config
        .compaction
        .timeout_seconds,
        300
    );
    assert!(resolve(
        Some(r#"{"compaction":{"timeout_seconds":1800}}"#),
        None,
        None
    )
    .is_ok());
    let retry = &schema()["properties"]["compaction"]["properties"]["retry_limit"];
    assert_eq!(retry["default"], 2);
    assert_eq!(retry["maximum"], 2);
    let field = &schema()["properties"]["compaction"]["properties"]["timeout_seconds"];
    assert_eq!(field["default"], 900);
    assert_eq!(field["maximum"], 1800);
}
#[test]
fn a_key_set_twice_is_refused_with_its_line() {
    let file = "{\n  \"sandbox\": { \"network\": \"block\" },\n  \"sandbox\": { \"writable_paths\": [] },\n}";
    let error = format!("{:#}", resolve(Some(file), None, None).unwrap_err());
    assert!(error.contains("\"sandbox\" is set twice"), "{error}");
    let issue = check_file(file).unwrap();
    assert_eq!(issue.line, Some(3));
    // Nested too; the same key in sibling objects or array items is fine.
    assert!(check_file(r#"{"browser":{"enabled":true,"enabled":false}}"#).is_some());
    assert!(check_file(r#"{"approval":{"rules":[{"tool":"*","action":"allow"},{"tool":"*","action":"ask"}]},"sandbox":{"network":"allow"}}"#).is_none());
    assert!(check_file(r#"{"models":{"a":{"thinking":true},"b":{"thinking":false}}}"#).is_none());
    // A key-like string value does not count as a key.
    assert!(check_file(
        r#"{"compaction":{"summary_model":"compaction"},"web":{"search_url":"web"}}"#
    )
    .is_none());
}
#[test]
fn jsonc_layers_and_validation() {
    let c = resolve(
        Some(
            "{\"compaction\":{\"timeout_seconds\":90,},//comment\n\"context_token_budget\":10000}",
        ),
        Some("{\"context_token_budget\":12000}"),
        None,
    )
    .unwrap();
    assert_eq!(c.config.compaction.timeout_seconds, 90);
    assert_eq!(
        c.config.context_token_budget,
        Some(ContextTokenBudget::Tokens(12000))
    );
    assert_eq!(c.provenance["context_token_budget"], "project");
    assert!(resolve(Some("{\"unknown\":1}"), None, None).is_err());
    assert!(parse("{/*").is_err());
    assert!(resolve(None, Some("{\"approval\":7}"), None).is_err());
    assert!(resolve(Some("{\"logging\":{\"level\":\"verbose\"}}"), None, None).is_err());
    assert_eq!(c.provenance["compaction.retry_limit"], "default");
    assert_eq!(
        parse("{\"url\":\"https://x/a/*b\"}").unwrap()["url"],
        "https://x/a/*b"
    );
}
#[test]
fn project_guidance_prefers_agents_md_and_maps_the_other_docs() {
    let root = tempfile::tempdir().unwrap();
    let config = RuntimeConfig::default();
    assert_eq!(project_guidance(&config, root.path()), "");
    std::fs::write(root.path().join("CLAUDE.md"), "claude").unwrap();
    assert!(project_guidance(&config, root.path()).contains("claude"));
    std::fs::write(root.path().join("AGENTS.md"), "agents").unwrap();
    std::fs::create_dir_all(root.path().join("docs/deep/deeper/deepest")).unwrap();
    std::fs::write(root.path().join("docs/a.md"), "x").unwrap();
    std::fs::write(root.path().join("docs/deep/deeper/deepest/far.md"), "x").unwrap();
    std::fs::create_dir_all(root.path().join(".git")).unwrap();
    std::fs::write(root.path().join(".git/x.md"), "x").unwrap();
    let text = project_guidance(&config, root.path());
    assert!(
        text.contains("agents") && !text.contains("claude\n"),
        "{text}"
    );
    assert!(
        text.contains("docs/a.md") && text.contains("CLAUDE.md"),
        "{text}"
    );
    assert!(!text.contains("far.md") && !text.contains(".git"), "{text}");
}

#[test]
fn project_guidance_is_bounded_and_never_fails_a_run() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("AGENTS.md"), "x".repeat(200_000)).unwrap();
    assert!(project_guidance(&RuntimeConfig::default(), root.path()).len() < 70_000);
    std::fs::write(root.path().join("AGENTS.md"), [0xff, 0xfe]).unwrap();
    assert_eq!(project_guidance(&RuntimeConfig::default(), root.path()), "");
}

/// Scale: a huge tree costs a bounded walk and a bounded list.
#[test]
fn the_docs_map_stays_bounded_on_a_big_project() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("AGENTS.md"), "a").unwrap();
    for folder in 0..40 {
        let dir = root.path().join(format!("d{folder}"));
        std::fs::create_dir_all(&dir).unwrap();
        for file in 0..40 {
            std::fs::write(dir.join(format!("f{file}.md")), "x").unwrap();
        }
    }
    let (small, _) = docs_map(root.path());
    assert!(small.len() <= MAX_MAPPED_DOCS, "{}", small.len());
    let (_, visited) = docs_map(root.path());
    assert!(visited <= MAX_MAP_VISITS, "{visited}");
}

#[test]
fn instructions_are_bounded_project_references() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("AGENTS.md"), "Preserve tests.").unwrap();
    let config = resolve(
        Some(r#"{"instructions":["AGENTS.md"],"skills":{"paths":["local-skills"]}}"#),
        None,
        None,
    )
    .unwrap()
    .config;
    assert!(load_instructions(&config, root.path())
        .unwrap()
        .contains("Preserve tests."));
    assert!(resolve(Some(r#"{"instructions":["../outside.md"]}"#), None, None).is_err());
    assert!(resolve(Some(r#"{"skills":{"paths":["/tmp/skills"]}}"#), None, None).is_err());
    std::fs::write(root.path().join("AGENTS.md"), "x".repeat(65537)).unwrap();
    assert!(load_instructions(&config, root.path()).is_err());
}

#[test]
fn context_budget_is_auto_or_a_cap_through_every_layer() {
    // Omitted everywhere: no override, the app setting (Auto by default) applies.
    let none = resolve(None, None, None).unwrap();
    assert_eq!(none.config.context_token_budget, None);
    assert!(!none.provenance.contains_key("context_token_budget"));
    // A user cap, then a project "auto" back to Auto, then a run cap.
    let user = r#"{"context_token_budget":500000}"#;
    let project = r#"{"context_token_budget":"auto"}"#;
    let layered = resolve(Some(user), Some(project), None).unwrap();
    assert_eq!(
        layered.config.context_token_budget,
        Some(ContextTokenBudget::Auto)
    );
    assert_eq!(layered.provenance["context_token_budget"], "project");
    let run = resolve(
        Some(user),
        Some(project),
        Some(r#"{"context_token_budget":64000}"#),
    )
    .unwrap();
    assert_eq!(
        run.config.context_token_budget,
        Some(ContextTokenBudget::Tokens(64000))
    );
    assert_eq!(run.provenance["context_token_budget"], "run");
    assert_eq!(
        ContextTokenBudget::Auto.cap(),
        crate::context_window::AUTO_CONTEXT_CEILING
    );
    assert_eq!(ContextTokenBudget::Tokens(64000).cap(), 64000);
    assert_eq!(
        serde_json::to_value(ContextTokenBudget::Auto).unwrap(),
        "auto"
    );
    assert_eq!(
        serde_json::to_value(ContextTokenBudget::Tokens(9000)).unwrap(),
        9000
    );
    assert!(resolve(Some(r#"{"context_token_budget":"Auto"}"#), None, None).is_err());
}

#[test]
fn legacy_section_max_tokens_still_loads_but_is_not_written() {
    let resolved = resolve(
        Some(r#"{"compaction":{"section_max_tokens":2048}}"#),
        None,
        None,
    )
    .unwrap();
    assert_eq!(resolved.config.compaction.section_max_tokens, 2048);
    let written = serde_json::to_value(&resolved.config).unwrap();
    assert!(written["compaction"].get("section_max_tokens").is_none());
    assert_eq!(CompactionConfig::default().section_max_tokens, 4096);
}
