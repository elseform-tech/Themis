//! The agent's mouse, drags, page reading and the looks check.

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn the_person_is_never_held_up_by_an_agent_action_in_progress() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
    // A busy page: it polls for six seconds, so its network never goes quiet while the
    // agent's open settles.
    Mock::given(path("/busy"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "<!doctype html><title>Busy</title><input id=q aria-label=Query autofocus>
             <script>const until = Date.now() + 6000;
             (function poll() { if (Date.now() < until) fetch('/ping').finally(() => setTimeout(poll, 150)); })();</script>",
            "text/html",
        ))
        .mount(&site)
        .await;
    Mock::given(path("/ping"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&site)
        .await;
    Mock::given(path("/other"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw("<!doctype html><title>Other</title>", "text/html"),
        )
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-busy", project.path());
    chat.open("about:blank").await.unwrap();
    chat.start_screencast(800, 600, 1.0, true).await.unwrap();
    let agent = {
        let chat = Arc::clone(&chat);
        let url = format!("{}/busy", site.uri());
        tokio::spawn(async move { chat.open(&url).await })
    };
    // Wait for the agent's page to commit (slow runners take over a second to get there),
    // then give it a moment so its open is settling on the page's endless polling.
    let waiting = std::time::Instant::now();
    while !chat.state().await.url.ends_with("/busy") {
        assert!(
            waiting.elapsed() < std::time::Duration::from_secs(10),
            "the agent's page never committed: {}",
            chat.state().await.url
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let opened = std::time::Instant::now();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(!agent.is_finished(), "the agent's open is still settling");

    // Everything the person does answers at once while the agent's action runs.
    let quick = std::time::Duration::from_millis(500);
    let started = std::time::Instant::now();
    let state = chat.state().await;
    assert!(state.url.ends_with("/busy"), "{}", state.url);
    chat.user_input(&themis_core::browser::UserInput::Text {
        text: "hello".into(),
    })
    .await
    .unwrap();
    assert!(chat.devtools_url().await.is_some());
    assert!(started.elapsed() < quick, "took {:?}", started.elapsed());
    assert!(!agent.is_finished());

    agent.await.unwrap().unwrap();
    // The agent is not held up either: a loaded page's endless polling stops counting
    // after a moment, long before it ends.
    assert!(
        opened.elapsed() < std::time::Duration::from_secs(4),
        "took {:?}",
        opened.elapsed()
    );
    let typed = chat.snapshot(None, false).await.unwrap().snapshot;
    assert!(
        typed.contains("hello"),
        "the person's typing reached the page:\n{typed}"
    );

    // A typed address goes at once, too.
    let started = std::time::Instant::now();
    chat.open_for_user(&format!("{}/other", site.uri()))
        .await
        .unwrap();
    assert!(started.elapsed() < quick, "took {:?}", started.elapsed());
    hub.close_chat("chat-busy").await;
}

/// Dragging is real mouse input the page can't tell from a person's, whichever way the page
/// listens: native HTML5 drag and drop, or plain mouse events.
#[tokio::test(flavor = "multi_thread")]
async fn the_agent_drags_with_the_mouse_on_html5_and_mouse_event_pages() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
    Mock::given(path("/board"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><title>Board</title><body style="margin:0">
<button id="card" draggable="true" style="position:fixed;left:20px;top:20px;width:120px;height:40px">Card one</button>
<button id="done" style="position:fixed;left:300px;top:20px;width:200px;height:200px">Done column</button>
<p id="out" style="position:fixed;top:300px">waiting</p>
<script>
const out = document.getElementById('out'), done = document.getElementById('done');
document.getElementById('card').addEventListener('dragstart', e => { e.dataTransfer.setData('text/plain', 'card-1'); });
done.addEventListener('dragover', e => e.preventDefault());
done.addEventListener('drop', e => { e.preventDefault(); out.textContent = 'dropped ' + e.dataTransfer.getData('text/plain') + ' trusted ' + e.isTrusted; });
</script></body>"#,
            "text/html",
        ))
        .mount(&site)
        .await;
    Mock::given(path("/slider"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><title>Slider</title><body style="margin:0;height:500px">
<div id="knob" style="position:fixed;left:50px;top:50px;width:30px;height:30px;background:#888"></div>
<p id="out" style="position:fixed;top:300px">waiting</p>
<script>
let held = false, moves = 0;
const knob = document.getElementById('knob'), out = document.getElementById('out');
knob.addEventListener('mousedown', e => { held = true; });
document.addEventListener('mousemove', e => { if (held && e.buttons === 1) { moves++; knob.style.left = e.clientX + 'px'; } });
document.addEventListener('mouseup', e => { if (held) out.textContent = 'let go at ' + e.clientX + ' after moves ' + (moves > 1 ? 'many' : moves) + ' trusted ' + e.isTrusted; held = false; });
</script></body>"#,
            "text/html",
        ))
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-drag", project.path());
    let tools = themis_core::browser::tools::browser_tools(
        Arc::new(FnHook::new(|_: &ToolAction| Approval::AllowOnce)),
        themis_core::browser::tools::BrowserScope {
            permissions: themis_core::tools::site_permissions::SitePermissions::new(
                Default::default(),
                None,
            ),
            chat: Arc::clone(&chat),
            cancel: None,
            media: None,
            screenshots: Default::default(),
        },
    );
    let tool = |name: &str| tools.iter().find(|tool| tool.name() == name).unwrap();

    // By refs, on a page using native drag and drop.
    let opened = tool("browser_open")
        .execute(json!({"url": format!("{}/board", site.uri())}))
        .await
        .unwrap();
    let snapshot = opened["snapshot"].as_str().unwrap();
    let (card, done) = (
        ref_of(snapshot, "Card one\""),
        ref_of(snapshot, "Done column\""),
    );
    let dropped = tool("browser_drag")
        .execute(json!({"from_reference": card, "to_reference": done}))
        .await
        .unwrap();
    let text = dropped["snapshot"].as_str().unwrap();
    assert!(text.contains("dropped card-1 trusted true"), "{text}");

    // By points, on a page using plain mouse events; a model's zero points beside refs are ignored.
    tool("browser_open")
        .execute(json!({"url": format!("{}/slider", site.uri())}))
        .await
        .unwrap();
    let slid = tool("browser_drag")
        .execute(json!({"from_x": 65, "from_y": 65, "to_x": 400, "to_y": 65}))
        .await
        .unwrap();
    let text = slid["snapshot"].as_str().unwrap();
    assert!(
        text.contains("let go at 400 after moves many trusted true"),
        "{text}"
    );

    // Nothing to go on, half a point, or a point off the page: told, and nothing happens.
    for args in [
        json!({}),
        json!({"from_x": 10, "from_y": 10}),
        json!({"from_x": 10, "to_x": 20, "to_y": 20}),
        json!({"from_x": 10, "from_y": 10, "to_x": 99999, "to_y": 10}),
    ] {
        assert!(
            tool("browser_drag").execute(args.clone()).await.is_err(),
            "{args}"
        );
    }

    // Holding the button needs somewhere to hold it, and letting go needs a press.
    let up = tool("browser_press")
        .execute(json!({"key": "mouseUp"}))
        .await
        .unwrap_err()
        .to_string();
    assert!(up.contains("mouseDown"), "{up}");
}

/// The agent can't see a pointer, so results say what it is over and what a drag did. Plain
/// `div`s that can be dragged have no role, and without a ref it could only guess at them.
#[tokio::test(flavor = "multi_thread")]
async fn the_agent_reads_what_is_under_the_mouse_and_what_a_drag_did() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
    Mock::given(path("/cards"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><title>Cards</title><body style="margin:0">
<div id="todo" class="column" style="position:fixed;left:0;top:0;width:200px;height:300px">
  <div id="card" class="task-card" draggable="true" style="margin:40px 10px;height:40px">Write the report</div>
</div>
<div id="done" class="column" style="position:fixed;left:300px;top:0;width:200px;height:300px">Done</div>
<script>
const done = document.getElementById('done'), card = document.getElementById('card');
card.addEventListener('dragstart', e => e.dataTransfer.setData('text/plain', 'x'));
done.addEventListener('dragover', e => e.preventDefault());
done.addEventListener('drop', e => { e.preventDefault(); done.appendChild(card); });
</script></body>"#,
            "text/html",
        ))
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-mouse", project.path());
    let tools = themis_core::browser::tools::browser_tools(
        Arc::new(FnHook::new(|_: &ToolAction| Approval::AllowOnce)),
        themis_core::browser::tools::BrowserScope {
            permissions: themis_core::tools::site_permissions::SitePermissions::new(
                Default::default(),
                None,
            ),
            chat: Arc::clone(&chat),
            cancel: None,
            media: None,
            screenshots: Default::default(),
        },
    );
    let tool = |name: &str| tools.iter().find(|tool| tool.name() == name).unwrap();
    let opened = tool("browser_open")
        .execute(json!({"url": format!("{}/cards", site.uri())}))
        .await
        .unwrap();

    // A draggable div gets a ref it can use, named by its text.
    let snapshot = opened["snapshot"].as_str().unwrap();
    assert!(
        snapshot.contains("draggable \"Write the report\" ["),
        "{snapshot}"
    );
    let card = ref_of(snapshot, "Write the report\"");

    // Over nothing, and over the card: it reads which, without a screenshot.
    let moved = tool("browser_move")
        .execute(json!({"x": 100, "y": 60}))
        .await
        .unwrap();
    let under = moved["under_mouse"].as_str().unwrap();
    assert!(
        under.contains("task-card") && under.contains("Write the report"),
        "{under}"
    );
    assert!(under.contains("draggable"), "{under}");
    assert_eq!(chat.state().await.pointer, Some((100.0, 60.0)));

    // A miss says so: it pressed on the column, and no drag began.
    let missed = tool("browser_drag")
        .execute(json!({"from_x": 100, "from_y": 250, "to_x": 400, "to_y": 150}))
        .await
        .unwrap();
    let result = missed["drag"].as_str().unwrap();
    assert!(result.contains("column"), "{result}");
    assert!(result.contains("nothing was dragged"), "{result}");

    // A hit names both ends and the outcome, by ref.
    let snapshot = tool("browser_snapshot")
        .execute(json!({}))
        .await
        .unwrap()
        .to_string();
    let done = {
        let after = tool("browser_drag")
            .execute(json!({"from_reference": card, "to_x": 400, "to_y": 150}))
            .await
            .unwrap();
        after["drag"].as_str().unwrap().to_owned()
    };
    assert!(done.contains("Write the report"), "{done} after {snapshot}");
    assert!(done.contains("native drag"), "{done}");
    assert!(
        done.contains("dropped on") && done.contains("column"),
        "{done}"
    );
    assert_eq!(chat.state().await.pointer, Some((400.0, 150.0)));
}

/// What agents reach for, taken from what one tried in a real run: elements by selector or text,
/// as Playwright takes them, and the mouse held down by hand.
#[tokio::test(flavor = "multi_thread")]
async fn the_agent_aims_by_selector_and_can_hold_the_mouse_down() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
    Mock::given(path("/board"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(BOARD, "text/html"))
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-aim-selector", project.path());
    let tools = themis_core::browser::tools::browser_tools(
        Arc::new(FnHook::new(|_: &ToolAction| Approval::AllowOnce)),
        themis_core::browser::tools::BrowserScope {
            permissions: themis_core::tools::site_permissions::SitePermissions::new(
                Default::default(),
                None,
            ),
            chat: Arc::clone(&chat),
            cancel: None,
            media: None,
            screenshots: Default::default(),
        },
    );
    let tool = |name: &str| tools.iter().find(|tool| tool.name() == name).unwrap();
    let url = format!("{}/board", site.uri());
    let open = || async {
        tool("browser_open")
            .execute(json!({"url": url}))
            .await
            .unwrap()
    };

    // By text and by CSS, for either end.
    open().await;
    let moved = tool("browser_drag")
        .execute(json!({"from_reference": "text=Write the report", "to_reference": "#done"}))
        .await
        .unwrap();
    assert!(
        moved["snapshot"].as_str().unwrap().contains("card in Done"),
        "{moved}"
    );

    tool("browser_navigate")
        .execute(json!({"action": "reload"}))
        .await
        .unwrap();
    let moved = tool("browser_drag")
        .execute(json!({"from_reference": ".task-card", "to_reference": "text=Done"}))
        .await
        .unwrap();
    assert!(
        moved["snapshot"].as_str().unwrap().contains("card in Done"),
        "{moved}"
    );

    // Nothing matching says so, and names what to do.
    let missing = tool("browser_drag")
        .execute(json!({"from_reference": "text=No such card", "to_reference": "#done"}))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        missing.contains("No such card") && missing.contains("browser_snapshot"),
        "{missing}"
    );

    // Clicking takes them too.
    tool("browser_navigate")
        .execute(json!({"action": "reload"}))
        .await
        .unwrap();
    tool("browser_click")
        .execute(json!({"reference": "text=Write the report"}))
        .await
        .unwrap();

    // The mouse by hand, as the agent tried: move, press, move, let go.
    tool("browser_navigate")
        .execute(json!({"action": "reload"}))
        .await
        .unwrap();
    tool("browser_move")
        .execute(json!({"x": 100, "y": 80}))
        .await
        .unwrap();
    let down = tool("browser_press")
        .execute(json!({"key": "mouseDown"}))
        .await
        .unwrap();
    assert!(
        down["under_mouse"]
            .as_str()
            .unwrap()
            .contains("Write the report"),
        "{down}"
    );
    tool("browser_move")
        .execute(json!({"x": 400, "y": 150}))
        .await
        .unwrap();
    let up = tool("browser_press")
        .execute(json!({"key": "mouseUp"}))
        .await
        .unwrap();
    assert!(
        up["snapshot"].as_str().unwrap().contains("card in Done"),
        "{up}"
    );
    assert!(up["drag"].as_str().unwrap().contains("native drag"), "{up}");

    // Having let go, moving is just moving again, with no button held.
    tool("browser_move")
        .execute(json!({"x": 10, "y": 10}))
        .await
        .unwrap();
}

/// The page's containers, where they are and how to name them, so nothing has to be guessed;
/// and whether a canvas drew anything, which a model that can't see has no other way to tell.
#[tokio::test(flavor = "multi_thread")]
async fn the_agent_reads_the_pages_structure_instead_of_guessing() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
    Mock::given(path("/skeleton"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(SKELETON, "text/html"))
        .mount(&site)
        .await;
    let many = format!(
        "<!doctype html><title>Many</title><body>{}</body>",
        "<div class=\"row\">x</div>".repeat(5000)
    );
    Mock::given(path("/many"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(many, "text/html"))
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-skeleton", project.path());
    let tools = themis_core::browser::tools::browser_tools(
        Arc::new(FnHook::new(|_: &ToolAction| Approval::AllowOnce)),
        themis_core::browser::tools::BrowserScope {
            permissions: themis_core::tools::site_permissions::SitePermissions::new(
                Default::default(),
                None,
            ),
            chat: Arc::clone(&chat),
            cancel: None,
            media: None,
            screenshots: Default::default(),
        },
    );
    let tool = |name: &str| tools.iter().find(|tool| tool.name() == name).unwrap();
    let opened = tool("browser_open")
        .execute(json!({"url": format!("{}/skeleton", site.uri())}))
        .await
        .unwrap();
    assert!(opened.get("layout").is_none(), "only when asked: {opened}");

    let read = tool("browser_snapshot")
        .execute(json!({"layout": true}))
        .await
        .unwrap();
    let layout = read["layout"].as_str().unwrap();
    let line = |needle: &str| {
        layout
            .lines()
            .find(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("no line with {needle} in:\n{layout}"))
            .to_owned()
    };

    // Containers the outline leaves out, nested, with where they are.
    assert!(line("section.column").contains("(0,50"), "{layout}");
    let done = line("\"Done\"");
    assert!(
        done.contains("(300,50") && done.contains("200×300"),
        "{layout}"
    );
    assert!(line("task-card").contains("(draggable)"), "{layout}");
    assert!(
        line("task-card").starts_with("    "),
        "nested under its column:\n{layout}"
    );

    // Canvases say what they drew.
    assert!(
        line("canvas#empty").contains("0×0") && line("canvas#empty").contains("nothing drawn"),
        "{layout}"
    );
    assert!(line("canvas#blank").contains("blank"), "{layout}");
    assert!(line("canvas#drawn").contains("has drawing"), "{layout}");

    // What is hidden isn't listed, and a field's value never appears.
    assert!(!layout.contains("Hidden words"), "{layout}");
    assert!(!layout.contains("hunter2"), "{layout}");

    // The selector printed for a line is one the tools take.
    let selector = done.rsplit("→ ").next().unwrap().trim().to_owned();
    let moved = tool("browser_drag")
        .execute(json!({"from_reference": "#card", "to_reference": selector}))
        .await
        .unwrap();
    assert!(
        moved["snapshot"].as_str().unwrap().contains("card in Done"),
        "{moved}"
    );

    // A huge page stays bounded, and says so.
    tool("browser_open")
        .execute(json!({"url": format!("{}/many", site.uri())}))
        .await
        .unwrap();
    let read = tool("browser_snapshot")
        .execute(json!({"layout": true}))
        .await
        .unwrap();
    let layout = read["layout"].as_str().unwrap();
    assert!(
        layout.lines().count() <= 202,
        "{} lines",
        layout.lines().count()
    );
    assert!(
        layout.contains("more not shown"),
        "{}",
        &layout[layout.len().saturating_sub(120)..]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_looks_check_finds_what_broke_trailboard_and_nothing_on_a_clean_page() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
    Mock::given(path("/trail/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(TRAILBOARD, "text/html; charset=utf-8"),
        )
        .mount(&site)
        .await;
    Mock::given(path("/trail/style.css"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(TRAILBOARD_CSS, "text/css"))
        .mount(&site)
        .await;
    Mock::given(path("/trail/theme.css"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&site)
        .await;
    Mock::given(path("/cut/"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(CUT_OFF, "text/html; charset=utf-8"))
        .mount(&site)
        .await;
    Mock::given(path("/clean/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(CLEAN_BOARD, "text/html; charset=utf-8"),
        )
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-looks", project.path());
    let tools = themis_core::browser::tools::browser_tools(
        Arc::new(FnHook::new(|_: &ToolAction| Approval::AllowOnce)),
        themis_core::browser::tools::BrowserScope {
            permissions: themis_core::tools::site_permissions::SitePermissions::new(
                Default::default(),
                None,
            ),
            chat: Arc::clone(&chat),
            cancel: None,
            media: None,
            screenshots: Default::default(),
        },
    );
    let tool = |name: &str| {
        tools
            .iter()
            .find(|tool| tool.name() == name)
            .unwrap_or_else(|| panic!("{name}"))
    };
    let dark = "window.matchMedia('(prefers-color-scheme: dark)').matches";
    let scheme =
        || async { chat.dev_console(Some(dark), 0, false).await.unwrap()["value"].clone() };

    let opened = tool("browser_open")
        .execute(json!({"url": format!("{}/trail/", site.uri())}))
        .await
        .unwrap();
    let before = scheme().await;
    let looks = tool("browser_check_looks")
        .execute(json!({}))
        .await
        .unwrap();
    let text = looks.to_string();
    assert_eq!(looks["untrusted"], true, "{looks:#}");
    assert!(looks["problems"].as_u64().unwrap() >= 6, "{looks:#}");
    // One list of the result as plain text.
    let list = |name: &str| {
        looks[name]
            .as_array()
            .map(|lines| {
                lines
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    };
    let page = list("page");
    assert!(page.contains("quirks mode"), "{looks:#}");
    assert!(
        page.contains("theme.css") && page.contains("404"),
        "{looks:#}"
    );
    assert!(page.contains("board state failed to load"), "{looks:#}");
    // The dropped rule names its file, line and the line itself.
    assert!(
        page.contains("style.css")
            && page.contains("line 1")
            && page.contains("// part of the project"),
        "{looks:#}"
    );
    // The comment line shown as page text, whatever the colour scheme.
    let both = list("both");
    assert!(
        both.contains("source code") && both.contains("// part of the project"),
        "{looks:#}"
    );
    // The button is unreadable in light mode only, and named the way the snapshot names it.
    let light = list("light_only");
    let snapshot = opened["snapshot"].as_str().unwrap();
    let add = ref_of(snapshot, "\"Add task\"");
    assert!(
        light.contains("button \"Add task\"") && light.contains(&format!("[{add}]")),
        "{looks:#}"
    );
    assert!(!list("dark_only").contains("Add task"), "{looks:#}");
    assert!(!both.contains("Add task"), "{looks:#}");
    assert!(text.len() < 8_000, "bounded: {} bytes", text.len());
    // The page is back in the scheme it had.
    assert_eq!(scheme().await, before);

    tool("browser_open")
        .execute(json!({"url": format!("{}/cut/", site.uri())}))
        .await
        .unwrap();
    let cut = tool("browser_check_looks")
        .execute(json!({}))
        .await
        .unwrap();
    let found = cut["both"]
        .as_array()
        .map(|lines| {
            lines
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    for expected in [
        "past it): div.banner \"Spring sale banner\"",
        "text cut off",
        "control with no visible text or edge: button \"Save\"",
        "field with no visible edge: textbox \"Search\"",
        "faint control text",
    ] {
        assert!(found.contains(expected), "{expected} in {cut:#}");
    }
    assert!(
        cut["page"].to_string().contains("scrolls sideways"),
        "{cut:#}"
    );

    tool("browser_open")
        .execute(json!({"url": format!("{}/clean/", site.uri())}))
        .await
        .unwrap();
    let clean = tool("browser_check_looks")
        .execute(json!({}))
        .await
        .unwrap();
    assert_eq!(clean["problems"], 0, "{clean:#}");
    assert_eq!(scheme().await, before);

    // A big page is looked at only up to the cap, and says so.
    let big = format!(
        "<!doctype html><title>Big</title>{}",
        "<p>row</p>".repeat(6_000)
    );
    Mock::given(path("/big/"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(big, "text/html"))
        .mount(&site)
        .await;
    tool("browser_open")
        .execute(json!({"url": format!("{}/big/", site.uri())}))
        .await
        .unwrap();
    let big = tool("browser_check_looks")
        .execute(json!({}))
        .await
        .unwrap();
    assert_eq!(
        big["checked"], "the first 5000 elements, in light and dark",
        "{big:#}"
    );
    hub.close_chat("chat-looks").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_looks_check_never_shows_field_values_or_tokens_in_urls() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
    Mock::given(path("/sec/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(SECRET_FORM, "text/html; charset=utf-8"),
        )
        .mount(&site)
        .await;
    Mock::given(path("/sec/style.css"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(SECRET_CSS, "text/css"))
        .mount(&site)
        .await;
    Mock::given(path("/sec/gone.css"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-looks-secrets", project.path());
    let tools = themis_core::browser::tools::browser_tools(
        Arc::new(FnHook::new(|_: &ToolAction| Approval::AllowOnce)),
        themis_core::browser::tools::BrowserScope {
            permissions: themis_core::tools::site_permissions::SitePermissions::new(
                Default::default(),
                None,
            ),
            chat: Arc::clone(&chat),
            cancel: None,
            media: None,
            screenshots: Default::default(),
        },
    );
    let tool = |name: &str| {
        tools
            .iter()
            .find(|tool| tool.name() == name)
            .unwrap_or_else(|| panic!("{name}"))
    };
    tool("browser_open")
        .execute(json!({"url": format!("{}/sec/", site.uri())}))
        .await
        .unwrap();
    let looks = tool("browser_check_looks")
        .execute(json!({}))
        .await
        .unwrap();
    let text = looks.to_string();
    for secret in [
        "hunter2",
        "sk9f8e7d6c5b4a39281706f5e4d3c2b1a0zz",
        "plainish-key-value-0454",
        "tok-stylesheet-0451",
        "tok-missing-0452",
        "frag-0453",
    ] {
        assert!(!text.contains(secret), "{secret} leaked: {looks:#}");
    }
    // The problems are still reported.
    assert!(text.matches("invisible text").count() >= 3, "{looks:#}");
    assert!(
        text.contains("style.css line 1") && text.contains("gone.css (HTTP 404)"),
        "{looks:#}"
    );
    hub.close_chat("chat-looks-secrets").await;
}
