//! The built-in browser against a real Chromium and a local test page: the
//! snapshot, refs, typing, clicking, dropdowns, keys, notes, password masking,
//! dialogs, history and tabs. Skipped (with a message) when no Chromium is
//! installed.

use std::sync::{Arc, Mutex};

#[path = "../browser_support/mod.rs"]
mod browser_support;
use browser_support::{act, frame_colour, frame_where, page_once};
use std::time::Duration;

use serde_json::{json, Value};
use themis_core::browser::{BrowserConfig, BrowserHub};
use themis_core::tools::{Approval, FnHook, ToolAction};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

mod mouse_and_looks;
mod pages_and_stream;

const FORM: &str = r#"<!doctype html><html><head><title>Sign up · Acme</title></head><body>
<h1>Create your account</h1>
<form id="f" onsubmit="event.preventDefault(); submitIt();">
  <label for="email">Email</label><input id="email" type="email">
  <label for="pw">Password</label><input id="pw" type="password" value="hunter2-secret">
  <label for="country">Country</label>
  <select id="country"><option value="">Pick one</option><option value="de">Germany</option><option value="fr">France</option></select>
  <button type="submit">Create account</button>
</form>
<p id="out"></p>
<a href="/second">Next page</a>
<button onclick="if (confirm('Delete everything?')) { document.getElementById('out').textContent = 'deleted'; } else { document.getElementById('out').textContent = 'kept'; }">Danger</button>
<script>
function submitIt() {
  const email = document.getElementById('email').value;
  const country = document.getElementById('country').value;
  document.getElementById('out').textContent = 'Welcome ' + email + ' from ' + country;
  console.error('signup tracker failed');
}
</script></body></html>"#;

/// Saves media into a test folder.
struct TempMedia(std::path::PathBuf);

impl themis_core::browser::media::MediaSink for TempMedia {
    fn save(&self, name: &str, bytes: &[u8]) -> Result<String, String> {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        Ok(path.to_string_lossy().into_owned())
    }
}

// One Chrome at a time: on GPU-less CI runners, Chromes started together
// starve each other and an animated page paints only a few times a second.
static ONE_BROWSER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn hub(root: &std::path::Path) -> Option<Arc<BrowserHub>> {
    let hub = BrowserHub::new(BrowserConfig {
        root: root.to_path_buf(),
        // A chosen engine, for checking one Chrome build; otherwise whatever is installed.
        executable: std::env::var_os("THEMIS_TEST_BROWSER").map(Into::into),
        headless: true,
    });
    if hub.executable().is_none() {
        eprintln!("skipping: no Chromium browser installed");
        return None;
    }
    Some(hub)
}

/// The ref (`e12`) on the line naming `needle`; it follows the name directly.
fn ref_of(snapshot: &str, needle: &str) -> String {
    let line = snapshot
        .lines()
        .find(|line| line.contains(needle) && line.contains("[e"))
        .unwrap_or_else(|| panic!("no line with {needle} in:\n{snapshot}"));
    let after = &line[line.find(needle).unwrap() + needle.len()..];
    let reference = after
        .trim_start()
        .strip_prefix('[')
        .and_then(|rest| rest.split(']').next())
        .filter(|r| r.starts_with('e') && r[1..].chars().all(|c| c.is_ascii_digit()))
        .unwrap_or_else(|| panic!("no ref after {needle}: {line}"));
    reference.to_owned()
}

/// The next frame: its pixel size, the page's CSS size it names, and its JPEG bytes.
async fn next_frame(
    updates: &mut tokio::sync::broadcast::Receiver<themis_core::browser::BrowserUpdate>,
) -> (usize, usize, u32, u32, usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(themis_core::browser::BrowserUpdate::Frame {
                data,
                width,
                height,
            }) = updates.recv().await
            {
                let (_, w, h) = frame_colour(&data);
                return (w, h, width, height, data.len() * 3 / 4);
            }
        }
    })
    .await
    .expect("a frame")
}

/// The actions the browser tool offers.
fn offered(tools: &[Box<dyn themis_core::tools::ToolT>]) -> Vec<String> {
    serde_json::from_value(tools[0].args_schema()["properties"]["action"]["enum"].clone()).unwrap()
}

/// Waits for a frame and the page `title` after a navigation, as the Workspace view sees them.
async fn follows(
    updates: &mut tokio::sync::broadcast::Receiver<themis_core::browser::BrowserUpdate>,
    title: &str,
    after: &str,
) {
    let (mut frame, mut titled) = (false, false);
    let followed = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while !(frame && titled) {
            match updates.recv().await {
                Ok(themis_core::browser::BrowserUpdate::Frame { .. }) => frame = true,
                Ok(themis_core::browser::BrowserUpdate::State(state)) => {
                    titled |= state.title == title;
                }
                _ => {}
            }
        }
    })
    .await;
    // On failure, also say what the stream did (counters are shared with tests alongside).
    assert!(
        followed.is_ok(),
        "after {after}: frame {frame}, title {title:?} {titled}; {:?}",
        themis_core::browser::stream_stats()
    );
}

const BOARD: &str = r#"<!doctype html><title>Board</title><body style="margin:0">
<div class="column todo" style="position:fixed;left:0;top:0;width:200px;height:300px"><h3>To Do</h3>
  <div id="card" class="task-card" draggable="true" style="height:40px">Write the report</div></div>
<div id="done" class="column" style="position:fixed;left:300px;top:0;width:200px;height:300px"><h3>Done</h3></div>
<p id="out" style="position:fixed;top:320px">card in To Do</p>
<script>
const done = document.getElementById('done'), card = document.getElementById('card'), out = document.getElementById('out');
card.addEventListener('dragstart', e => e.dataTransfer.setData('text/plain', 'x'));
done.addEventListener('dragover', e => e.preventDefault());
done.addEventListener('drop', e => { e.preventDefault(); done.appendChild(card); out.textContent = 'card in Done'; });
</script></body>"#;

const SKELETON: &str = r#"<!doctype html><title>Skeleton</title><body style="margin:0">
<main class="board" style="position:fixed;left:0;top:50px;width:600px;height:300px">
  <section class="column todo" style="position:absolute;left:0;top:0;width:200px;height:300px"><h3>To Do</h3>
    <div id="card" class="task-card" draggable="true" style="height:40px">Write the report</div></section>
  <section class="column" style="position:absolute;left:300px;top:0;width:200px;height:300px"><h3>Done</h3></section>
</main>
<div style="display:none" class="never-shown">Hidden words</div>
<input type="password" class="secret" value="hunter2-secret" style="position:fixed;top:0;left:0">
<canvas id="empty"></canvas><canvas id="blank" width="50" height="50"></canvas>
<canvas id="drawn" width="50" height="50"></canvas>
<p id="out" style="position:fixed;top:400px">card in To Do</p>
<script>
document.getElementById('empty').width = 0; document.getElementById('empty').height = 0;
const pen = document.getElementById('drawn').getContext('2d'); pen.fillStyle = 'red'; pen.fillRect(5, 5, 10, 10);
const done = document.querySelectorAll('.column')[1], card = document.getElementById('card');
card.addEventListener('dragstart', e => e.dataTransfer.setData('text/plain', 'x'));
done.addEventListener('dragover', e => e.preventDefault());
done.addEventListener('drop', e => { e.preventDefault(); done.appendChild(card); document.getElementById('out').textContent = 'card in Done'; });
</script></body>"#;

/// Trailboard as an agent built it: a comment line before the doctype (shown on the page, and
/// the page drops to quirks mode), the same `//` line first in its stylesheet (which kills the
/// first rule, the columns' background), an "Add task" button whose text is its background
/// colour in light mode only, a stylesheet that is missing and a console error.
const TRAILBOARD: &str = r#"// part of the project
<!doctype html><html><head><title>Trailboard</title>
<link rel="stylesheet" href="/trail/style.css">
<link rel="stylesheet" href="/trail/theme.css">
</head><body>
<main class="board">
  <section class="column"><h2>To do</h2><p>Plan the route</p></section>
  <section class="column"><h2>Done</h2><p>Buy boots</p></section>
</main>
<button class="add">Add task</button>
<script>console.error('board state failed to load');</script>
</body></html>"#;

const TRAILBOARD_CSS: &str = r#"// part of the project
.column { background: #e8eefc; padding: 8px; }
body { font-family: sans-serif; background: #ffffff; color: #1f2937; }
.board { display: flex; gap: 12px; }
button.add { background: #2563eb; color: #2563eb; border: none; padding: 8px 12px; }
@media (prefers-color-scheme: dark) {
  body { background: #111827; color: #e5e7eb; }
  button.add { color: #ffffff; }
}
"#;

/// The same board done right, with things that only look like problems: a code sample, a
/// visually hidden label, a strip that scrolls sideways on purpose and a dark mode.
const CLEAN_BOARD: &str = r#"<!doctype html><html><head><title>Clean board</title>
<meta name="color-scheme" content="light dark">
<style>
body { font-family: sans-serif; background: #ffffff; color: #1f2937; margin: 16px; }
.column { background: #e8eefc; padding: 8px; }
.board { display: flex; gap: 12px; }
button.add { background: #2563eb; color: #ffffff; border: none; padding: 8px 12px; }
.visually-hidden { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; }
.strip { overflow-x: auto; white-space: nowrap; width: 300px; }
.strip span { display: inline-block; width: 200px; }
input { border: 1px solid #6b7280; background: #ffffff; color: #111827; }
@media (prefers-color-scheme: dark) {
  body { background: #111827; color: #e5e7eb; }
  .column { background: #1f2937; }
  input { background: #1f2937; color: #f9fafb; border-color: #9ca3af; }
}
</style></head><body>
<main class="board">
  <section class="column"><h2>To do</h2><p>Plan the route</p></section>
  <section class="column"><h2>Done</h2><p>Buy boots</p></section>
</main>
<label><span class="visually-hidden">New task</span><input placeholder="New task"></label>
<button class="add">Add task</button>
<pre><code>// part of the project
function add() {}</code></pre>
<div class="strip"><span>One</span><span>Two</span><span>Three</span></div>
</body></html>"#;

/// The other kinds of problem the check measures, one each.
const CUT_OFF: &str = r#"<!doctype html><html><head><title>Cut off</title><style>
body { margin: 0; background: #ffffff; color: #111827; font-family: sans-serif; }
.banner { width: 3000px; background: #fde68a; }
.note { height: 18px; width: 120px; overflow: hidden; }
.ghost { background: transparent; border: 0; color: transparent; width: 60px; height: 30px; }
.bare { border: 0; background: #ffffff; color: #111827; }
.faint { background: #ffffff; color: #b0b0b0; border: 1px solid #111827; }
</style></head><body>
<div class="banner">Spring sale banner</div>
<p class="note">This note is far too long to fit inside its small box, so most of it is hidden from view.</p>
<button class="ghost">Save</button>
<input class="bare" aria-label="Search">
<button class="faint">Cancel</button>
</body></html>"#;

/// Secrets on a page with problems: a filled password field and secret-looking text fields,
/// all unreadable, and stylesheets whose URLs carry a token.
const SECRET_FORM: &str = r#"<!doctype html><html><head><title>Secrets</title>
<link rel="stylesheet" href="/sec/style.css?token=tok-stylesheet-0451">
<link rel="stylesheet" href="/sec/gone.css?token=tok-missing-0452#frag-0453">
</head><body>
<input type="password" value="hunter2-looks-secret" class="hush">
<input name="note" value="sk9f8e7d6c5b4a39281706f5e4d3c2b1a0zz" class="hush">
<input name="api_key" value="plainish-key-value-0454" class="hush">
</body></html>"#;

const SECRET_CSS: &str =
    "// a comment line\n.gone { color: red; }\n.hush { color: #ffffff; background: #ffffff; border: 1px solid #000000; }\n";
