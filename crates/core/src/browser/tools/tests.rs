use super::args::{action_of, FIELDS};
use super::input::*;
use super::pages::*;
use super::*;
use crate::tools::site_permissions::{browser_permission, Permission};
use crate::tools::ToolCallError;
use proptest::prelude::*;

#[test]
fn a_ref_of_nothing_is_no_ref() {
    for empty in ["", " ", "null", "NULL", "\"null\"", "none", "undefined"] {
        assert_eq!(model_reference(empty), "", "{empty}");
    }
    assert_eq!(model_reference(" \"e14\" "), "e14");
    assert_eq!(model_reference("nullable"), "nullable");
}

#[tokio::test]
async fn a_stopped_run_drops_the_browser_step_in_progress() {
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let guard = Dropped(Arc::clone(&dropped));
    let step = async move {
        let _guard = guard;
        std::future::pending::<Result<(), String>>().await
    };
    let flag = Arc::clone(&cancel);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        flag.store(true, Ordering::SeqCst);
    });
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        until_stopped(Some(&cancel), step),
    )
    .await
    .expect("a stopped step returns promptly");
    assert_eq!(result, Err("stopped by you".to_owned()));
    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(
        until_stopped(Some(&cancel), async { Ok(1) }).await.ok(),
        Some(1)
    );
    assert_eq!(
        until_stopped(None, async { Ok::<_, String>(2) }).await,
        Ok(2)
    );
}

struct NoMedia;
impl MediaSink for NoMedia {
    fn save(&self, _: &str, _: &[u8]) -> Result<String, String> {
        Err("unused".into())
    }
}

/// The browser tool for a run: with somewhere to save media or not, Developer mode on or off.
fn tool(media: bool, developer_mode: bool, screenshots: ScreenshotsToModel) -> Box<dyn ToolT> {
    let hub = super::super::BrowserHub::new(super::super::BrowserConfig {
        root: std::env::temp_dir(),
        executable: None,
        headless: true,
    });
    let config = crate::configuration::BrowserConfig {
        developer_mode,
        ..Default::default()
    };
    let mut tools = browser_tools(
        Arc::new(crate::tools::AllowAllHook),
        BrowserScope {
            permissions: SitePermissions::new(config, None),
            chat: hub.chat("schema-test", &std::env::temp_dir()),
            cancel: None,
            media: media.then(|| Arc::new(NoMedia) as Arc<dyn MediaSink>),
            screenshots,
        },
    );
    assert_eq!(tools.len(), 1, "one browser tool");
    tools.remove(0)
}

fn offered(tool: &dyn ToolT) -> Vec<String> {
    tool.args_schema()["properties"]["action"]["enum"]
        .as_array()
        .expect("an action enum")
        .iter()
        .map(|name| name.as_str().unwrap().to_owned())
        .collect()
}

/// Parses `args` the way `action` does, without running it.
fn parses(action: Action, args: Value) -> Result<(), ToolCallError> {
    match action {
        Action::Open => parse_action::<OpenArgs>(action, args).map(|_| ()),
        Action::Navigate => parse_action::<NavigateArgs>(action, args).map(|_| ()),
        Action::Snapshot => parse_action::<SnapshotArgs>(action, args).map(|_| ()),
        Action::CheckLooks => parse_action::<CheckLooksArgs>(action, args).map(|_| ()),
        Action::Screenshot => parse_action::<ScreenshotArgs>(action, args).map(|_| ()),
        Action::Record => parse_action::<RecordArgs>(action, args).map(|_| ()),
        Action::Click => parse_action::<ClickArgs>(action, args).map(|_| ()),
        Action::Type => parse_action::<TypeArgs>(action, args).map(|_| ()),
        Action::Select => parse_action::<SelectArgs>(action, args).map(|_| ()),
        Action::Press => parse_action::<PressArgs>(action, args).map(|_| ()),
        Action::Scroll => parse_action::<ScrollArgs>(action, args).map(|_| ()),
        Action::Move => parse_action::<MoveArgs>(action, args).map(|_| ()),
        Action::Drag => parse_action::<DragArgs>(action, args).map(|_| ()),
        Action::Devtools => parse_action::<super::devtools::DevtoolsArgs>(action, args).map(|_| ()),
    }
}

/// The fields each action's own arguments read, by their names on the wire.
fn fields_of(action: Action) -> Vec<String> {
    let value = match action {
        Action::Open => serde_json::to_value(OpenArgs::default()),
        Action::Navigate => serde_json::to_value(NavigateArgs::default()),
        Action::Snapshot => serde_json::to_value(SnapshotArgs::default()),
        Action::CheckLooks => serde_json::to_value(CheckLooksArgs::default()),
        Action::Screenshot => serde_json::to_value(ScreenshotArgs::default()),
        Action::Record => serde_json::to_value(RecordArgs::default()),
        Action::Click => serde_json::to_value(ClickArgs::default()),
        Action::Type => serde_json::to_value(TypeArgs::default()),
        Action::Select => serde_json::to_value(SelectArgs::default()),
        Action::Press => serde_json::to_value(PressArgs::default()),
        Action::Scroll => serde_json::to_value(ScrollArgs::default()),
        Action::Move => serde_json::to_value(MoveArgs::default()),
        Action::Drag => serde_json::to_value(DragArgs::default()),
        Action::Devtools => serde_json::to_value(super::devtools::DevtoolsArgs::default()),
    };
    value
        .unwrap()
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect()
}

/// A model fills in exactly what the schema advertises; every one of those names must parse
/// for every action (a live run once lost three calls to `reference` in the schema but `ref`
/// in the parser), and `ref` is accepted too.
#[test]
fn every_advertised_argument_parses_for_every_action_and_ref_is_accepted_too() {
    let tool = tool(true, true, ScreenshotsToModel::Off);
    let schema = tool.args_schema();
    assert_eq!(schema["required"], json!(["action"]), "{schema}");
    let sample = |schema: &Value| match schema["type"].as_str() {
        Some("boolean") => json!(true),
        Some("integer" | "number") => json!(1),
        Some("array") => json!(["x"]),
        _ => json!("e1"),
    };
    let properties = schema["properties"].as_object().unwrap();
    for (action, name, _) in args::ACTIONS {
        let mut args: serde_json::Map<String, Value> = properties
            .iter()
            .map(|(field, spec)| (field.clone(), sample(spec)))
            .collect();
        args.insert("action".into(), json!(name));
        let parsed = parses(action, Value::Object(args));
        assert!(parsed.is_ok(), "{name}: {parsed:?} from {schema}");
    }
    let click: ClickArgs = parse_action(Action::Click, json!({"ref": "e4"})).unwrap();
    assert_eq!(click.reference, "e4");
    let click: ClickArgs = parse_action(Action::Click, json!({"reference": "e5"})).unwrap();
    assert_eq!(click.reference, "e5");
}

/// Each action's own fields are in the schema, each marked with the actions that read it, and
/// the schema has no field no action reads.
#[test]
fn every_field_names_the_actions_that_read_it() {
    let tool = tool(true, true, ScreenshotsToModel::Off);
    let schema = tool.args_schema();
    let properties = schema["properties"].as_object().unwrap();
    for (action, name, _) in args::ACTIONS {
        for field in fields_of(action) {
            let readers = FIELDS
                .iter()
                .find(|(known, _)| *known == field)
                .unwrap_or_else(|| panic!("{name} reads {field}, which the schema lacks"))
                .1;
            assert!(readers.contains(&action), "{field} does not list {name}");
            assert!(properties.contains_key(&field), "{field} not advertised");
        }
    }
    for (field, readers) in FIELDS {
        let description = properties[*field]["description"].as_str().unwrap();
        for reader in *readers {
            assert!(
                fields_of(*reader).contains(&(*field).to_owned()),
                "{field} lists {} but it does not read it",
                reader.name()
            );
            assert!(
                description.contains(reader.name()),
                "{field}'s description does not name {}: {description}",
                reader.name()
            );
        }
    }
    assert_eq!(
        properties.len(),
        FIELDS.len() + 1,
        "only fields an action reads"
    );
}

/// Each action keeps the risk and the Settings › Browser permission of the tool it replaced,
/// and a page's download keeps its own: the table the approval rules, site grants and
/// Settings › Browser read.
#[test]
fn every_action_keeps_the_risk_and_permission_of_the_tool_it_replaced() {
    use Permission::{Browsing, Debugging, Downloads};
    use RiskLevel::{Network, Read};
    let before: [(&str, &str, RiskLevel, Permission); 15] = [
        ("browser_open", "open", Network, Browsing),
        ("browser_navigate", "navigate", Network, Browsing),
        ("browser_snapshot", "snapshot", Read, Browsing),
        ("browser_click", "click", Network, Browsing),
        ("browser_type", "type", Network, Browsing),
        ("browser_select", "select", Network, Browsing),
        ("browser_press", "press", Network, Browsing),
        ("browser_scroll", "scroll", Read, Browsing),
        ("browser_move", "move", Network, Browsing),
        ("browser_drag", "drag", Network, Browsing),
        ("browser_check_looks", "check_looks", Read, Browsing),
        ("browser_screenshot", "screenshot", Read, Browsing),
        ("browser_record", "record", Read, Browsing),
        // Its risk rises with what a call does; reading is Read.
        ("browser_devtools", "devtools", Read, Debugging),
        ("browser_download", DOWNLOAD, DOWNLOAD_RISK, Downloads),
    ];
    for (old, key, risk, permission) in before {
        assert_eq!(browser_permission(key), Some(permission), "{old} → {key}");
        if key == DOWNLOAD {
            assert_eq!(risk, Network, "{old}");
            continue;
        }
        let (action, ..) = args::ACTIONS
            .iter()
            .find(|(_, name, _)| *name == key)
            .unwrap_or_else(|| panic!("{old} has no action {key}"));
        assert_eq!(action.risk(), risk, "{old} → {key}");
        assert_eq!(action.name(), key);
    }
    assert_eq!(
        crate::tools::site_permissions::BROWSER_KEYS.len(),
        before.len()
    );
}

/// What a run lacks is not offered: screenshots and recordings without a place to save them,
/// `devtools` without Developer mode, and the fields only they read.
#[test]
fn the_schema_offers_only_what_the_run_has() {
    let all = tool(true, true, ScreenshotsToModel::Off);
    assert_eq!(offered(all.as_ref()).len(), 14);
    assert_eq!(all.name(), "browser");
    let plain = tool(false, false, ScreenshotsToModel::Off);
    let names = offered(plain.as_ref());
    for missing in ["screenshot", "record", "devtools"] {
        assert!(!names.contains(&missing.to_owned()), "{missing}");
        assert!(!plain.description().contains(&format!("- {missing}:")));
    }
    assert_eq!(names.len(), 11);
    let properties = plain.args_schema()["properties"].clone();
    for gone in [
        "panel",
        "full_page",
        "operation",
        "max_seconds",
        "expression",
    ] {
        assert!(properties.get(gone).is_none(), "{gone} kept");
    }
    // `url` stays for open, `reference` for click.
    assert!(properties.get("url").is_some() && properties.get("reference").is_some());
    let devtools = tool(false, true, ScreenshotsToModel::Off);
    assert!(offered(devtools.as_ref()).contains(&"devtools".to_owned()));
    assert!(devtools.args_schema()["properties"]
        .get("operation")
        .is_some());
    assert!(devtools.description().contains("- devtools:"));
}

/// A model that reads text only is told it cannot see a screenshot; one that reads images is
/// told it gets the image, with the same schema, and each result says whether the image follows.
#[test]
fn the_screenshot_action_tells_each_model_what_it_gets() {
    let text = tool(true, false, ScreenshotsToModel::Off);
    assert!(text.description().contains("You cannot see it yourself"));
    assert!(text
        .description()
        .ends_with(crate::prompts::TOOL_BROWSER_RECORD));
    let seeing = tool(true, false, ScreenshotsToModel::Seen);
    assert!(seeing
        .description()
        .contains(crate::prompts::TOOL_BROWSER_SCREENSHOT_SEEING));
    assert!(!seeing.description().contains("cannot see"));
    assert_eq!(seeing.args_schema(), text.args_schema());
    assert!(text.description().starts_with(crate::prompts::TOOL_BROWSER));
    assert_eq!(
        screenshot_note(ScreenshotsToModel::Off, false),
        crate::prompts::TOOL_SCREENSHOT_NOTE
    );
    assert_eq!(
        screenshot_note(ScreenshotsToModel::Seen, true),
        crate::prompts::TOOL_SCREENSHOT_SEEN_NOTE
    );
    assert_eq!(
        screenshot_note(ScreenshotsToModel::Seen, false),
        crate::prompts::TOOL_SCREENSHOT_UNATTACHED_NOTE
    );
}

/// A missing, mistyped or unknown action is refused naming `action` and listing the actions
/// the run offers; so is one the run does not offer.
#[tokio::test]
async fn an_unknown_action_is_refused_with_the_valid_ones() {
    let tool = tool(false, false, ScreenshotsToModel::Off);
    let valid =
        "open, navigate, snapshot, click, type, select, press, scroll, move, drag, check_looks";
    for (args, start) in [
        (json!({}), "browser: argument 'action' is missing"),
        (
            json!({"action": 3}),
            "browser: argument 'action' is invalid",
        ),
        (
            json!({"action": "hover"}),
            "browser: unknown action 'hover'",
        ),
        (
            json!({"action": "browser_click"}),
            "browser: unknown action 'browser_click'",
        ),
        (
            json!({"action": "devtools", "panel": "console"}),
            "browser: unknown action 'devtools'",
        ),
        (
            json!({"action": "screenshot"}),
            "browser: unknown action 'screenshot'",
        ),
    ] {
        let error = tool.execute(args.clone()).await.unwrap_err().to_string();
        assert!(error.contains(start), "{args}: {error}");
        assert!(error.contains(valid), "{args}: {error}");
    }
}

/// Each action reaches its own code: its own checks answer, named after it, before any page
/// is needed.
#[tokio::test]
async fn each_action_reaches_its_own_code() {
    let tool = tool(true, true, ScreenshotsToModel::Off);
    for (args, error) in [
        (
            json!({"action": "open", "url": "ftp://example.com"}),
            "browser open: ",
        ),
        (
            json!({"action": "navigate"}),
            "browser navigate: invalid arguments: missing field `direction`",
        ),
        (
            json!({"action": " Click "}),
            "browser click: give a ref, or x and y",
        ),
        (
            json!({"action": "click", "x": 3}),
            "browser click: a point needs both x and y",
        ),
        (
            json!({"action": "type", "reference": "e1"}),
            "browser type: argument 'reference' is invalid: missing field `text`",
        ),
        (
            json!({"action": "select", "reference": "e1", "values": []}),
            "browser select: give 1..50 values",
        ),
        (
            json!({"action": "press"}),
            "browser press: invalid arguments: missing field `key`",
        ),
        (
            json!({"action": "scroll"}),
            "browser scroll: invalid arguments: missing field `direction`",
        ),
        (
            json!({"action": "move"}),
            "browser move: give either x and y, or dx and dy",
        ),
        (
            json!({"action": "drag"}),
            "browser drag: give the start as a ref, or both its x and y",
        ),
        (
            json!({"action": "record"}),
            "browser record: invalid arguments: missing field `operation`",
        ),
        (
            json!({"action": "devtools"}),
            "browser devtools: invalid arguments: missing field `panel`",
        ),
        (
            json!({"action": "snapshot", "layout": "yes"}),
            "browser snapshot: argument 'layout'",
        ),
        (
            json!({"action": "screenshot", "full_page": 2}),
            "browser screenshot: argument 'full_page'",
        ),
    ] {
        let got = tool.execute(args.clone()).await.unwrap_err().to_string();
        assert!(got.contains(error), "{args}: {got}");
    }
}

fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        any::<f64>().prop_map(|n| json!(n)),
        ".{0,12}".prop_map(Value::from),
        prop::sample::select(
            args::ACTIONS
                .iter()
                .map(|(_, name, _)| (*name).to_owned())
                .collect::<Vec<_>>()
        )
        .prop_map(Value::from),
    ];
    leaf.prop_recursive(3, 24, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::from),
            prop::collection::hash_map("[a-z_]{1,14}", inner, 0..6)
                .prop_map(|map| Value::Object(map.into_iter().collect())),
        ]
    })
}

proptest! {
    /// Whatever a model sends, picking the action and parsing its fields never panics; a
    /// refusal names the tool (and the action, once one is picked), and an accepted call names
    /// an offered action, read without case or surrounding space.
    #[test]
    fn any_model_input_picks_an_offered_action_or_is_refused(
        mut args in json_value(),
        action in prop::option::of(json_value()),
        fields in prop::collection::hash_map(
            prop::sample::select(FIELDS.iter().map(|(name, _)| (*name).to_owned()).collect::<Vec<_>>()),
            json_value(),
            0..6,
        ),
    ) {
        let offered: Vec<Action> = args::ACTIONS.iter().map(|(action, ..)| *action).collect();
        if let Some(object) = args.as_object_mut() {
            object.extend(fields);
            if let Some(action) = action {
                object.insert("action".into(), action);
            }
        }
        match action_of(&args, &offered) {
            Ok(picked) => {
                let given = args["action"].as_str().unwrap().trim().to_ascii_lowercase();
                prop_assert_eq!(picked.name(), given);
                if let Err(error) = parses(picked, args.clone()) {
                    let error = error.to_string();
                    let prefix = format!("browser {}: ", picked.name());
                    prop_assert!(error.contains(&prefix), "{}", error);
                }
            }
            Err(error) => {
                let error = error.to_string();
                prop_assert!(error.contains("browser: "), "{}", error);
                prop_assert!(error.contains("check_looks"), "{}", error);
            }
        }
    }
}
