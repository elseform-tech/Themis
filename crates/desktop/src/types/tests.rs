use super::*;
use serde::Serialize;
use serde_json::{json, Value};
use themis_core::runtime::ApprovalDecision as CoreApprovalDecision;
use themis_core::runtime::RunEvent;

fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("serializable")
}

#[test]
fn thread_events_serialize_exactly_like_ts() {
    let cases: Vec<(ThreadEvent, Value)> = vec![
        (
            ThreadEvent::Started {
                task: "do it".to_owned(),
                max_turns: 20,
            },
            json!({"kind": "started", "task": "do it", "max_turns": 20}),
        ),
        (
            ThreadEvent::AssistantText {
                text: "hi".to_owned(),
            },
            json!({"kind": "assistant_text", "text": "hi"}),
        ),
        (
            ThreadEvent::ToolStarted {
                tool: "shell".to_owned(),
                summary: "run".to_owned(),
                label: None,
            },
            json!({"kind": "tool_started", "tool": "shell", "summary": "run"}),
        ),
        (
            ThreadEvent::ToolStarted {
                tool: "mcp_0123456789abcdef01234567_browser_navigate".to_owned(),
                summary: "{}".to_owned(),
                label: Some("playwright · browser_navigate".to_owned()),
            },
            json!({"kind": "tool_started", "tool": "mcp_0123456789abcdef01234567_browser_navigate", "summary": "{}", "label": "playwright · browser_navigate"}),
        ),
        (
            ThreadEvent::ToolFinished {
                tool: "shell".to_owned(),
                ok: true,
                output: "result".to_owned(),
                media: None,
            },
            json!({"kind": "tool_finished", "tool": "shell", "ok": true, "output": "result"}),
        ),
        (
            ThreadEvent::ApprovalDecided {
                tool: "shell".to_owned(),
                decision: ApprovalDecision::Once,
            },
            json!({"kind": "approval_decided", "tool": "shell", "decision": "once"}),
        ),
        (
            ThreadEvent::ContextCompacting,
            json!({"kind": "context_compacting"}),
        ),
        (
            ThreadEvent::Finished {
                model: None,
                result: "done".to_owned(),
            },
            json!({"kind": "finished", "result": "done"}),
        ),
        (
            ThreadEvent::Failed {
                error: "boom".to_owned(),
            },
            json!({"kind": "failed", "error": "boom"}),
        ),
    ];
    for (event, expected) in cases {
        assert_eq!(to_value(&event), expected, "{event:?}");
    }
    // The remaining decision spellings.
    assert_eq!(to_value(&ApprovalDecision::Chat), json!("chat"));
    assert_eq!(to_value(&ApprovalDecision::Deny), json!("deny"));
    assert_eq!(
        serde_json::from_value::<ApprovalDecision>(json!("once")).unwrap(),
        ApprovalDecision::Once
    );
}

#[test]
fn run_event_mapping_covers_every_variant() {
    assert_eq!(
        ThreadEvent::from(RunEvent::Started {
            task: "t".to_owned(),
            max_turns: 3,
        }),
        ThreadEvent::Started {
            task: "t".to_owned(),
            max_turns: 3,
        }
    );
    assert_eq!(
        ThreadEvent::from(RunEvent::AssistantText("a".to_owned())),
        ThreadEvent::AssistantText {
            text: "a".to_owned()
        }
    );
    assert_eq!(
        ThreadEvent::from(RunEvent::ToolCallStarted {
            tool: "t".to_owned(),
            summary: "s".to_owned(),
        }),
        ThreadEvent::ToolStarted {
            tool: "t".to_owned(),
            summary: "s".to_owned(),
            label: None,
        }
    );
    assert_eq!(
        ThreadEvent::from(RunEvent::ToolCallFinished {
            tool: "t".to_owned(),
            ok: false,
            output: "failed".to_owned(),
        }),
        ThreadEvent::ToolFinished {
            tool: "t".to_owned(),
            ok: false,
            output: "failed".to_owned(),
            media: None,
        }
    );
    for (core, wire) in [
        (CoreApprovalDecision::AllowOnce, ApprovalDecision::Once),
        (CoreApprovalDecision::AllowAlways, ApprovalDecision::Chat),
        (CoreApprovalDecision::Deny, ApprovalDecision::Deny),
    ] {
        assert_eq!(
            ThreadEvent::from(RunEvent::ApprovalDecided {
                tool: "t".to_owned(),
                decision: core,
            }),
            ThreadEvent::ApprovalDecided {
                tool: "t".to_owned(),
                decision: wire,
            }
        );
    }
    assert_eq!(
        ThreadEvent::from(RunEvent::ContextCompacting),
        ThreadEvent::ContextCompacting
    );
    assert_eq!(
        ThreadEvent::from(RunEvent::Finished {
            result: "r".to_owned()
        }),
        ThreadEvent::Finished {
            model: None,
            result: "r".to_owned()
        }
    );
    assert_eq!(
        ThreadEvent::from(RunEvent::Failed {
            error: "e".to_owned()
        }),
        ThreadEvent::Failed {
            error: "e".to_owned()
        }
    );
}

#[test]
fn envelopes_use_snake_case_keys() {
    let envelope = ThreadEventEnvelope {
        thread_id: "t".to_owned(),
        run_id: "r".to_owned(),
        event: ThreadEvent::Finished {
            model: None,
            result: "ok".to_owned(),
        },
    };
    assert_eq!(
        to_value(&envelope),
        json!({
            "thread_id": "t",
            "run_id": "r",
            "event": {"kind": "finished", "result": "ok"},
        })
    );
    let request = ApprovalRequest {
        thread_id: "t".to_owned(),
        approval_id: "a".to_owned(),
        tool: "shell".to_owned(),
        summary: "run".to_owned(),
        risk: RiskLevel::Execute,
        grant_scope: Some(themis_core::tools::GrantScope {
            argv: vec!["cargo".to_owned(), "test".to_owned()],
            covers_further_args: true,
        }),
        grant_site: None,
        grantable: true,
        command: None,
        subagent: None,
        sensitive_files: Vec::new(),
        ask_first: false,
    };
    assert_eq!(
        to_value(&request),
        json!({
            "thread_id": "t",
            "approval_id": "a",
            "tool": "shell",
            "summary": "run",
            "risk": "execute",
            "grant_scope": {"argv": ["cargo", "test"], "covers_further_args": true},
            "grantable": true,
        })
    );
    // Requests from before chat grants had no flag and stay grantable.
    let legacy: ApprovalRequest = serde_json::from_value(json!({
        "thread_id": "t", "approval_id": "a", "tool": "shell", "summary": "run", "risk": "execute"
    }))
    .unwrap();
    assert!(legacy.grantable);
    assert_eq!(
        serde_json::from_value::<ApprovalDecision>(json!("always")).unwrap(),
        ApprovalDecision::Chat
    );
}

#[test]
fn diff_shapes_match_ts_and_omit_absent_optionals() {
    let hunk = DiffHunk {
        old_start: 1,
        old_lines: 2,
        new_start: 1,
        new_lines: 3,
        lines: vec![
            DiffLine {
                kind: DiffLineKind::Context,
                text: "same".to_owned(),
            },
            DiffLine {
                kind: DiffLineKind::Del,
                text: "old".to_owned(),
            },
            DiffLine {
                kind: DiffLineKind::Add,
                text: "new".to_owned(),
            },
        ],
    };
    assert_eq!(
        to_value(&hunk),
        json!({
            "old_start": 1,
            "old_lines": 2,
            "new_start": 1,
            "new_lines": 3,
            "lines": [
                {"kind": "context", "text": "same"},
                {"kind": "del", "text": "old"},
                {"kind": "add", "text": "new"},
            ],
        })
    );
    let file = DiffFile {
        path: "a.rs".to_owned(),
        old_path: None,
        status: DiffStatus::Modified,
        preexisting: false,
        hunks: vec![hunk],
        added: 1,
        removed: 0,
        too_large: false,
        hidden: false,
    };
    let json = to_value(&file);
    assert!(json.get("too_large").is_none());
    assert!(json.get("hidden").is_none());
    let hidden = DiffFile {
        hidden: true,
        ..file.clone()
    };
    assert_eq!(to_value(&hidden)["hidden"], true);
    assert_eq!(json["path"], "a.rs");
    assert_eq!(json["status"], "modified");
    assert!(json.get("old_path").is_none());
    let renamed = DiffFile {
        old_path: Some("old.rs".to_owned()),
        status: DiffStatus::Renamed,
        ..file.clone()
    };
    assert_eq!(to_value(&renamed)["old_path"], "old.rs");
    assert_eq!(to_value(&DiffStatus::Added), json!("added"));
    assert_eq!(to_value(&DiffStatus::Deleted), json!("deleted"));
    let unavailable = DiffState {
        available: false,
        reason: Some("not a git repo".to_owned()),
        files: vec![],
    };
    assert_eq!(
        to_value(&unavailable),
        json!({"available": false, "reason": "not a git repo", "files": []})
    );
    let no_reason = DiffState {
        available: true,
        reason: None,
        files: vec![],
    };
    assert!(to_value(&no_reason).get("reason").is_none());
}

#[test]
fn provider_serialization_roundtrips() {
    assert_eq!(to_value(&ProviderKind::Go), json!("go"));
    assert_eq!(
        serde_json::from_value::<ProviderKind>(json!("go")).unwrap(),
        ProviderKind::Go
    );
    // A retired provider in an older file reads as Go.
    assert_eq!(
        serde_json::from_value::<ProviderKind>(json!("retired-provider")).unwrap(),
        ProviderKind::Go
    );
}

#[test]
fn thread_info_and_merge_result_match_ts() {
    let info = ThreadInfo {
        last_activity_seq: 42,
        last_outcome: None,
        open_questions: 0,
        goal_status: None,
        id: "t".to_owned(),
        title: "hi".to_owned(),
        provider: ProviderKind::Go,
        model: "m".to_owned(),
        reasoning_effort: None,
        approval_mode: Default::default(),
        running: false,
        worktree_path: Some("/tmp/wt".to_owned()),
        branch: Some("themis/abc".to_owned()),
        base_branch: Some("main".to_owned()),
        recovered: true,
        skill_ids: vec!["skill-1".to_owned()],
        vision: None,
    };
    assert_eq!(
        to_value(&info),
        json!({
            "last_activity_seq": 42,
            "id": "t",
            "title": "hi",
            "provider": "go",
            "model": "m",
            "approval_mode": "custom",
            "running": false,
            "worktree_path": "/tmp/wt",
            "branch": "themis/abc",
            "base_branch": "main",
            "recovered": true,
            "skill_ids": ["skill-1"],
            "vision": null,
        })
    );
    let marked = ThreadInfo {
        vision: Some(ThreadVision {
            id: "v".to_owned(),
            name: "Rook".to_owned(),
            shape: "tall".to_owned(),
            color: "amber".to_owned(),
        }),
        ..info.clone()
    };
    assert_eq!(
        to_value(&marked)["vision"],
        json!({"id": "v", "name": "Rook", "shape": "tall", "color": "amber"})
    );
    // Absent worktrees serialize as `null`, exactly like the TS `string | null`.
    let plain = ThreadInfo {
        worktree_path: None,
        branch: None,
        base_branch: None,
        recovered: false,
        ..info
    };
    let json = to_value(&plain);
    assert_eq!(json["worktree_path"], Value::Null);
    assert_eq!(json["branch"], Value::Null);
    assert_eq!(json["base_branch"], Value::Null);
    assert_eq!(json["recovered"], json!(false));

    let merged = MergeResult {
        applied: true,
        applied_files: vec!["a.txt".to_owned()],
        conflicts: Vec::new(),
    };
    assert_eq!(
        to_value(&merged),
        json!({"applied": true, "applied_files": ["a.txt"], "conflicts": []})
    );
    let conflicted = MergeResult {
        applied: false,
        applied_files: Vec::new(),
        conflicts: vec!["b.txt".to_owned()],
    };
    assert_eq!(
        to_value(&conflicted),
        json!({"applied": false, "applied_files": [], "conflicts": ["b.txt"]})
    );
}

#[test]
fn settings_defaults_and_shapes_match_ts() {
    let settings = Settings::default();
    assert_eq!(settings.theme, ThemeMode::Dark);
    assert_eq!(settings.default_provider, ProviderKind::Go);
    assert_eq!(settings.default_model, "muse-spark-1.3-contributor");
    assert_eq!(settings.context_token_cap, 0);
    assert!(settings.recent_roots.is_empty());
    assert_eq!(settings.concurrency_limit, 3);
    assert!(settings.automations_enabled);
    assert!(!settings.completion_sound);
    assert!(!settings.completion_haptic);
    assert!(!settings.onboarded);
    assert_eq!(
        to_value(&settings),
        json!({
            "projects_directory": default_projects_directory(),
            "text_size": 12,
            "text_size_chosen": false,
            "theme_palette": "classic",
            "font_family": "system",
            "sidebar_hover": true,
            "check_sources_for_updates": true,
            "theme": "dark",
            "default_provider": "go",
            "default_model": "muse-spark-1.3-contributor",
            "max_total_turns": 200,
            "context_token_cap": 0,
            "approval_timeout_seconds": 600,
            "confirm_reads": false,
            "recent_roots": [],
            "concurrency_limit": 3,
            "automations_enabled": true,
            "completion_sound": false,
            "completion_haptic": false,
            "goal_notifications": true,
            "config_editor": "",
            "default_approval_mode": "auto",
            "go_secret": "OPENCODE_KEY",
            "onboarded": false,
        })
    );
    // A TS-style partial patch deserializes with the rest defaulted.
    let patch: SettingsPatch =
        serde_json::from_value(json!({"theme": "dark", "max_turns": 42})).unwrap();
    assert_eq!(patch.theme, Some(ThemeMode::Dark));
    // The retired `max_turns` from old clients is ignored.
    assert!(patch.context_token_cap.is_none());
    assert!(patch.default_provider.is_none());
    assert!(patch.default_model.is_none());
    assert!(patch.recent_roots.is_none());
    assert!(patch.concurrency_limit.is_none());
    assert!(patch.automations_enabled.is_none());
    assert!(patch.onboarded.is_none());
    let patch: SettingsPatch =
        serde_json::from_value(json!({"automations_enabled": false})).unwrap();
    assert_eq!(patch.automations_enabled, Some(false));
    let patch: SettingsPatch = serde_json::from_value(json!({"onboarded": true})).unwrap();
    assert_eq!(patch.onboarded, Some(true));
    // Settings files written before the new fields existed still load,
    // with automations enabled and onboarding not done.
    let legacy: Settings = serde_json::from_value(json!({
        "theme": "dark",
        "default_provider": "openai",
        "default_model": "gpt-x",
        "max_turns": 7,
    }))
    .unwrap();
    assert_eq!(legacy.theme, ThemeMode::Dark);
    assert!(legacy.recent_roots.is_empty());
    assert_eq!(legacy.concurrency_limit, 3);
    assert!(legacy.automations_enabled);
    assert!(
        !legacy.completion_sound,
        "a file without the field keeps the sound off"
    );
    assert!(!legacy.completion_haptic);
    assert!(legacy.goal_notifications);
    assert!(!legacy.onboarded);
    let status = SecretStatus { go: true };
    assert_eq!(to_value(&status), json!({"go": true}));
}

#[test]
fn skill_shapes_match_ts_and_convert_to_core() {
    let skill = Skill {
        id: "skill-1".to_owned(),
        name: "Rust".to_owned(),
        description: "clippy-clean code".to_owned(),
        instructions: "Run clippy.".to_owned(),
        allowed_tools: vec!["read_file".to_owned()],
        scripts: vec![SkillScript {
            name: "lint.sh".to_owned(),
            content: "echo hi".to_owned(),
        }],
    };
    assert_eq!(
        to_value(&skill),
        json!({
            "id": "skill-1",
            "name": "Rust",
            "description": "clippy-clean code",
            "instructions": "Run clippy.",
            "allowed_tools": ["read_file"],
            "scripts": [{"name": "lint.sh", "content": "echo hi"}],
        })
    );
    let input = SkillInput {
        name: "Rust".to_owned(),
        description: "clippy-clean code".to_owned(),
        instructions: "Run clippy.".to_owned(),
        allowed_tools: vec![],
        scripts: vec![],
    };
    let parsed: SkillInput = serde_json::from_value(to_value(&input)).unwrap();
    assert_eq!(parsed, input);
    // The core conversion preserves every field.
    let core = skill.core_skill();
    assert_eq!(core.id, "skill-1");
    assert_eq!(core.name, "Rust");
    assert_eq!(core.description, "clippy-clean code");
    assert_eq!(core.instructions, "Run clippy.");
    assert_eq!(core.allowed_tools, vec!["read_file".to_owned()]);
    assert_eq!(core.scripts.len(), 1);
    assert_eq!(core.scripts[0].name, "lint.sh");
    assert_eq!(core.scripts[0].content, "echo hi");
}

#[test]
fn automation_and_review_shapes_match_ts() {
    let automation = Automation {
        id: "a".to_owned(),
        name: "Nightly".to_owned(),
        project_root: "/tmp/repo".to_owned(),
        provider: ProviderKind::Go,
        model: "m".to_owned(),
        reasoning_effort: None,
        target_thread_id: None,
        skill_ids: vec!["s".to_owned()],
        interval_mins: 60,
        schedule: None,
        task: "check health".to_owned(),
        enabled: true,
        last_run_at: None,
        next_run_at: "2026-01-01T00:00:00Z".to_owned(),
        run_count: 0,
        origin: AutomationOrigin::User,
        watch: Default::default(),
        watch_state: Default::default(),
    };
    assert_eq!(
        to_value(&automation),
        json!({
            "id": "a",
            "name": "Nightly",
            "project_root": "/tmp/repo",
            "provider": "go",
            "model": "m",
            "skill_ids": ["s"],
            "interval_mins": 60,
            "task": "check health",
            "enabled": true,
            "last_run_at": null,
            "next_run_at": "2026-01-01T00:00:00Z",
            "run_count": 0,
        })
    );
    let with_effort = Automation {
        reasoning_effort: Some("medium".to_owned()),
        ..automation
    };
    assert_eq!(to_value(&with_effort)["reasoning_effort"], "medium");
    let item = ReviewItem {
        id: "r".to_owned(),
        automation_id: "a".to_owned(),
        thread_id: "t".to_owned(),
        created_at: "2026-01-01T01:00:00Z".to_owned(),
        title: "Automation Nightly".to_owned(),
        summary: "done".to_owned(),
        status: ReviewStatus::Pending,
        source: Default::default(),
    };
    assert_eq!(
        to_value(&item),
        json!({
            "id": "r",
            "automation_id": "a",
            "thread_id": "t",
            "created_at": "2026-01-01T01:00:00Z",
            "title": "Automation Nightly",
            "summary": "done",
            "status": "pending",
        })
    );
    assert_eq!(to_value(&ReviewStatus::Continued), json!("continued"));
    assert_eq!(to_value(&ReviewStatus::Dismissed), json!("dismissed"));
    assert_eq!(
        serde_json::from_value::<ReviewStatus>(json!("pending")).unwrap(),
        ReviewStatus::Pending
    );
    let now = RunAutomationNow {
        automation_id: "a".to_owned(),
        thread_id: "t".to_owned(),
        run_id: "r".to_owned(),
    };
    assert_eq!(
        to_value(&now),
        json!({"automation_id": "a", "thread_id": "t", "run_id": "r"})
    );
    assert_eq!(REVIEW_ITEM_NAME, "review-item-added");
}

#[test]
fn diagnostics_and_update_status_match_ts() {
    // `UpdateState` is kebab-case on the wire (`up-to-date`).
    for (state, wire) in [
        (UpdateState::Disabled, "disabled"),
        (UpdateState::UpToDate, "up-to-date"),
        (UpdateState::Available, "available"),
        (UpdateState::Error, "error"),
    ] {
        assert_eq!(to_value(&state), json!(wire));
        assert_eq!(
            serde_json::from_value::<UpdateState>(json!(wire)).unwrap(),
            state
        );
    }
    // Absent optionals are omitted, exactly like the TS optional fields.
    assert_eq!(
        to_value(&UpdateStatus::disabled("updates not configured")),
        json!({"state": "disabled", "message": "updates not configured"})
    );
    assert_eq!(
        to_value(&UpdateStatus {
            state: UpdateState::UpToDate,
            version: None,
            notes: None,
            message: None,
        }),
        json!({"state": "up-to-date"})
    );
    assert_eq!(
        to_value(&UpdateStatus {
            state: UpdateState::Available,
            version: Some("0.2.0".to_owned()),
            notes: Some("fixes".to_owned()),
            message: None,
        }),
        json!({"state": "available", "version": "0.2.0", "notes": "fixes"})
    );
    let diagnostics = Diagnostics {
        persistence: Vec::new(),
        recovery_notes: 0,
        app_version: "0.1.0".to_owned(),
        os: "macos".to_owned(),
        settings: Settings::default(),
        recent_errors: vec![DiagnosticsError {
            at: "2026-01-01T00:00:00Z".to_owned(),
            command: "open_project".to_owned(),
            message: "no such directory".to_owned(),
        }],
    };
    let json = to_value(&diagnostics);
    assert_eq!(json["app_version"], "0.1.0");
    assert_eq!(json["os"], "macos");
    assert_eq!(json["settings"]["onboarded"], false);
    assert_eq!(
        json["recent_errors"],
        json!([{
            "at": "2026-01-01T00:00:00Z",
            "command": "open_project",
            "message": "no such directory",
        }])
    );
    let roundtrip: Diagnostics = serde_json::from_value(json).unwrap();
    assert_eq!(roundtrip, diagnostics);
}
