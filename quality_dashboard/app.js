const $ = (selector, root = document) => root.querySelector(selector);
const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];
const viewNames = { overview: "Overview", changes: "Change register", tests: "Test register", complexity: "Code health", runs: "Run history" };
let snapshot = null;
let toastTimer;

const esc = (value) => String(value ?? "").replace(/[&<>"']/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char]);
const number = (value) => value == null ? "—" : new Intl.NumberFormat().format(value);
const shortDate = (value) => value ? new Date(value).toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" }) : "—";
const percent = (value) => value == null ? "—" : `${Number(value).toFixed(1)}%`;
const statusText = (value) => value === "passed" ? "Passed" : value === "failed" ? "Failed" : value === "blocked" ? "Blocked" : value === "running" ? "Running" : value === "unknown" ? "Unverified" : value === "skipped" ? "Skipped" : "Not run";
const statusClass = (value) => value === "passed" || value === "failed" || value === "blocked" || value === "running" ? value : "neutral";

function metricCard(label, value, caption, icon, unit = "") {
  return `<article class="metric-card"><div class="metric-top"><span>${esc(label)}</span><span class="metric-icon" aria-hidden="true">${esc(icon)}</span></div><div class="metric-value">${esc(value)}${unit ? `<span class="metric-unit">${esc(unit)}</span>` : ""}</div><div class="metric-caption">${esc(caption)}</div></article>`;
}

function table(headers, rows, empty = "No data to display") {
  if (!rows.length) return `<div class="table-empty">${esc(empty)}</div>`;
  return `<table><thead><tr>${headers.map((header) => `<th>${header}</th>`).join("")}</tr></thead><tbody>${rows.join("")}</tbody></table>`;
}

function testTotals(run, productOnly = false) {
  if (!run) return { passed: 0, failed: 0, skipped: 0, known: false };
  const checks = (run.checks || []).filter((check) => !productOnly || check.suite !== "dashboard");
  const known = checks.some((check) => check.passed != null || check.failed != null);
  return checks.reduce((sum, check) => ({
    passed: sum.passed + (Number(check.passed) || 0),
    failed: sum.failed + (Number(check.failed) || 0),
    skipped: sum.skipped + (Number(check.skipped) || 0),
    known,
  }), { passed: 0, failed: 0, skipped: 0, known });
}

function renderOverview(data) {
  const { repo, source, tests, complexity, coverage, runs, activeRun } = data;
  const productTotal = tests.overallTotal ?? tests.total;
  const overallTotal = tests.overallDiscoveredTotal ?? tests.discoveredTotal;
  const currentChanges = repo.changes.length;
  const latest = activeRun || runs.at(-1);
  const testCaption = `${number(productTotal)} product · ${number(tests.toolingTotal)} tooling`;
  $("#metricGrid").innerHTML = [
    metricCard("Repository changes", number(currentChanges), `${number(repo.workingChurn.added)} additions · ${number(repo.workingChurn.removed)} deletions`, "↗"),
    metricCard("Overall test definitions", number(overallTotal), testCaption, "✓"),
    metricCard("Product source lines", number(source.loc), `${number(source.productionLoc)} production · ${number(source.testLoc)} test-file lines`, "≋"),
    metricCard(
      "Avg. cyclomatic complexity",
      complexity.available ? complexity.averageCyclomatic.toFixed(2) : "Unavailable",
      complexity.available ? "AST average per source file" : "Install rust-code-analysis-cli",
      "⌁",
    ),
    metricCard("Product code churn", `${number(repo.monthChurn.added)} / ${number(repo.monthChurn.removed)}`, `${number(repo.monthChurn.commits)} app commits · last 30 days`, "↕", "+ / −"),
  ].join("");

  $("#branchName").textContent = repo.branch;
  $("#commitHash").textContent = repo.commit;
  const working = $("#workingState");
  working.textContent = repo.clean ? "Working tree clean" : `${currentChanges} changed ${currentChanges === 1 ? "file" : "files"}`;
  working.classList.toggle("dirty", !repo.clean);
  $("#changeBadge").textContent = number(currentChanges);

  const coverageContent = $("#coverageContent");
  if (coverage.available) {
    coverageContent.innerHTML = coverage.reports.map((report) => `<div class="coverage-report"><div class="coverage-ring" style="--coverage:${Number(report.percent)}%"><strong>${esc(percent(report.percent))}</strong></div><div class="coverage-copy"><strong>${esc(report.scope)} · ${number(report.hit)} of ${number(report.found)} lines covered</strong><span>LCOV report: <code>${esc(report.path)}</code></span></div></div>`).join("");
  } else {
    coverageContent.innerHTML = `<div class="coverage-report"><div class="coverage-ring"><strong>—</strong></div><div class="coverage-copy"><strong>No coverage report found</strong><span>Test totals do not measure coverage. Add an LCOV report at <code>coverage/lcov.info</code> or <code>web/coverage/lcov.info</code>.</span></div></div>`;
  }

  const latestRun = $("#latestRunContent");
  const runStatus = $("#runStatus");
  if (!latest) {
    $("#runCardTitle").textContent = "No recorded run";
    runStatus.className = "status-pill neutral";
    runStatus.textContent = "Not run";
    latestRun.innerHTML = `<div class="run-empty">Run the Rust and frontend suites to record pass/fail totals here.</div>`;
  } else {
    const state = activeRun ? "running" : latest.status;
    $("#runCardTitle").textContent = activeRun ? `Run ${latest.id}` : shortDate(latest.finishedAt);
    runStatus.className = `status-pill ${statusClass(state)}`;
    runStatus.textContent = statusText(state);
    const checks = activeRun ? activeRun.checks : latest.checks;
    latestRun.innerHTML = checks.length ? checks.map((check) => {
      const counts = check.passed == null ? check.status === "running" ? "Executing…" : "Counts unavailable" : `${number(check.passed)} passed · ${number(check.failed)} failed`;
      const suiteName = check.suite === "rust" ? "Product · Rust workspace" : check.suite === "dashboard" ? "Tooling · dashboard tests" : "Product · Frontend / Vitest";
      return `<div class="run-row"><span class="run-row-label">${esc(suiteName)}</span><span class="run-row-detail">${esc(counts)}</span></div>`;
    }).join("") : `<div class="run-empty">${number(tests.total)} discovered tests · ${activeRun ? "Starting checks…" : "No suite results"}</div>`;
  }
  const runLabel = $("#activeRunLabel");
  runLabel.textContent = activeRun ? `Running ${activeRun.currentSuite || "test checks"} · started ${shortDate(activeRun.startedAt)}` : latest ? `Last run ${shortDate(latest.finishedAt)}${latest.commit ? ` · ${latest.commit}` : ""}` : "No checks are running";
  $(".pulse-dot").classList.toggle("active", Boolean(activeRun));

  const changeRows = repo.changes.slice(0, 5).map((item) => `<tr><td class="path-cell">${esc(item.path)}</td><td><span class="status-code">${esc(item.status)}</span></td><td class="delta-add">+${number(item.added)}</td><td class="delta-remove">−${number(item.removed)}</td></tr>`);
  $("#overviewChanges").innerHTML = table(["Path", "Status", "+", "−"], changeRows, "No uncommitted changes");
  $("#overviewTests").innerHTML = [
    ["Unit / component · automated", tests.counts.unit], ["Integration · automated", tests.counts.integration],
    ["E2E · automated", tests.counts.e2e],
    ["Dashboard tooling", tests.toolingTotal],
  ].map(([label, count]) => `<div class="suite-line"><span class="suite-label"><i class="suite-bullet"></i>${esc(label)}</span><span class="suite-value">${number(count)}</span></div>`).join("");
}

function renderChanges(data) {
  const { repo } = data;
  $("#changeSummary").textContent = `${number(repo.changes.length)} changed ${repo.changes.length === 1 ? "file" : "files"} in the working tree`;
  $("#churnGrid").innerHTML = [
    metricCard("Commits · 30 days", number(repo.monthChurn.commits), "Current branch history", "◷"),
    metricCard("Lines added · 30 days", number(repo.monthChurn.added), "Committed code churn", "+"),
    metricCard("Lines removed · 30 days", number(repo.monthChurn.removed), "Committed code churn", "−"),
    metricCard("Uncommitted lines", `+${number(repo.workingChurn.added)} / −${number(repo.workingChurn.removed)}`, "Current tracked diff and new files", "↕"),
  ].join("");
  const filter = $("#changeFilter").value.trim().toLowerCase();
  const rows = repo.changes.filter((item) => `${item.path} ${item.status}`.toLowerCase().includes(filter)).map((item) => `<tr><td class="path-cell">${esc(item.path)}</td><td><span class="status-code">${esc(item.status)}</span></td><td class="delta-add">+${number(item.added)}</td><td class="delta-remove">−${number(item.removed)}</td></tr>`);
  $("#changesTable").innerHTML = table(["File", "Git status", "Lines added", "Lines removed"], rows, filter ? "No matching changes" : "Working tree is clean");
  const commits = repo.recentCommits.map((item) => `<tr><td class="commit-tag">${esc(item.commit)}</td><td>${esc(item.date)}</td><td>${esc(item.subject)}</td></tr>`);
  $("#commitTable").innerHTML = table(["Commit", "Date", "Subject"], commits, "No commits found");
}

function renderTests(data) {
  const { tests, runs, activeRun } = data;
  const productTotal = tests.overallTotal ?? tests.total;
  const overallTotal = tests.overallDiscoveredTotal ?? tests.discoveredTotal;
  const openSuites = new Set($$("#testsTable .test-case-details[open]").map((item) => item.dataset.suite));
  const last = activeRun || runs.at(-1);
  const totals = testTotals(last, true);
  const productSuites = tests.suites.filter((suite) => suite.scope === "product").length;
  $("#testGrid").innerHTML = [
    metricCard("Overall test definitions", number(overallTotal), `${number(productTotal)} product · ${number(tests.toolingTotal)} tooling`, "✓"),
    metricCard("Product end-to-end definitions", number(tests.counts.e2e), "Automated Rust and frontend tests", "↗"),
    metricCard("Dashboard tooling checks", number(tests.toolingTotal), "Automated dashboard tests", "⚙"),
    metricCard("Latest automated tests passed", totals.known ? number(totals.passed) : "—", last ? activeRun ? "Run in progress" : shortDate(last.finishedAt) : "Run suites to record results", "+"),
    metricCard("Latest automated tests failed", totals.known ? number(totals.failed) : "—", totals.known ? "Product suites only" : "No recorded product results", "!"),
    metricCard("Latest automated tests skipped", totals.known ? number(totals.skipped) : "—", totals.known ? "Product suites only" : "No recorded product results", "↷"),
  ].join("");
  const scopeFilter = $("#testScopeFilter").value;
  const kindFilter = $("#testTypeFilter").value;
  const query = $("#testFilter").value.trim().toLowerCase();
  const selected = tests.suites.filter((suite) => (scopeFilter === "all" || suite.scope === scopeFilter) && (kindFilter === "all" || suite.kind === kindFilter) && `${suite.path} ${suite.framework} ${suite.kind} ${suite.scope}`.toLowerCase().includes(query));
  const latestFor = (suite) => {
    const key = suite.framework === "Rust test harness" ? "rust" : suite.framework === "Vitest" ? "frontend" : "dashboard";
    const current = activeRun?.checks?.find((check) => check.suite === key);
    if (current) return current;
    return runs.slice().reverse().map((run) => run.checks?.find((check) => check.suite === key)).find(Boolean) || null;
  };
  const caseResultFor = (check, suite, name) => (check?.caseResults || []).find((result) => {
    if (suite.framework === "Rust test harness") return result.path === suite.path && result.name.endsWith(`::${name}`);
    if (suite.framework === "Vitest") return result.name.endsWith(name) || result.name.split(" > ").pop() === name;
    return result.name === name;
  });
  const rows = selected.map((suite) => {
    const check = latestFor(suite);
    const perFile = check?.fileResults?.[suite.path];
    const executedCases = (check?.caseResults || []).filter((result) => result.path === suite.path);
    const toolingSummary = suite.scope === "tooling" && check && check.status !== "running";
    const state = !check ? "neutral" : check.status === "running" ? "running" : perFile ? perFile.failed ? "failed" : perFile.passed ? "passed" : perFile.skipped ? "skipped" : "neutral" : toolingSummary ? check.status : "unknown";
    const outcome = !check ? "Not run" : perFile ? `${number(perFile.passed)} passed · ${number(perFile.failed)} failed · ${number(perFile.skipped)} skipped in file` : toolingSummary ? `Tooling suite ${check.status} · individual outcomes unavailable` : check.status === "running" ? "Run in progress · waiting for file-level events" : "Unverified · no test events mapped to this file";
    const cases = executedCases.length
      ? executedCases.map((result) => {
          const name = result.name.split(" > ").slice(1).join(" > ") || result.name;
          return `<li><code>${esc(name)}</code><span class="status-pill ${statusClass(result.status)}">${esc(statusText(result.status))}</span></li>`;
        }).join("")
      : suite.names.map((name) => {
          const result = caseResultFor(check, suite, name);
          const caseState = result?.status || (!check ? "neutral" : check.status === "running" ? "running" : "unknown");
          return `<li><code>${esc(name)}</code><span class="status-pill ${statusClass(caseState)}">${esc(statusText(caseState))}</span></li>`;
        }).join("");
    const summary = `<span class="status-pill ${statusClass(state)}">${esc(statusText(state))}</span><small class="suite-result-count">${esc(outcome)}</small>`;
    const detailLabel = executedCases.length ? `${number(executedCases.length)} executed test cases` : `${number(suite.tests)} declared test ${suite.tests === 1 ? "definition" : "definitions"}`;
    const details = `<details class="test-case-details" data-suite="${esc(suite.path)}" ${openSuites.has(suite.path) ? "open" : ""}><summary>Inspect ${detailLabel}</summary><ul class="test-case-list">${cases}</ul></details>`;
    return `<tr><td>${esc(suite.scope === "tooling" ? "Tooling" : "Product")}</td><td class="path-cell">${esc(suite.path)}</td><td>${esc(suite.framework)}</td><td>${esc(suite.kind === "e2e" ? "End-to-end" : suite.kind === "integration" ? "Integration" : suite.framework === "Vitest" ? "Unit / component" : "Unit")}</td><td>${number(suite.tests)}</td><td>${summary}${details}</td></tr>`;
  });
  $("#testsTable").innerHTML = table(["Scope", "Suite file", "Harness", "Type", "Definitions", "Latest result and test names"], rows, "No matching test suites");
}

function renderComplexity(data) {
  const { complexity } = data;
  if (!complexity.available) {
    $("#complexityGrid").innerHTML = metricCard(
      "AST metrics",
      "Unavailable",
      complexity.error || "Install rust-code-analysis-cli to calculate code health.",
      "!",
    );
    $("#complexityTable").innerHTML = table(
      ["Module", "LOC", "Cyclomatic avg", "Cognitive avg", "Mean function MI", "Index"],
      [],
      "Install rust-code-analysis-cli to analyze source files.",
    );
    return;
  }
  $("#complexityGrid").innerHTML = [
    metricCard("Cyclomatic complexity", complexity.averageCyclomatic.toFixed(2), "AST average per source file", "⌁"),
    metricCard("Cognitive complexity", complexity.averageCognitive.toFixed(2), "AST average per source file", "⋔"),
    metricCard("Maintainability index", `${complexity.averageMaintainability.toFixed(1)}`, "Mean function AST index · review signal", "◒", "/ 100"),
    metricCard("Files analyzed", number(complexity.filesAnalyzed), "App source files; standalone tests/tooling excluded", "▤"),
  ].join("");
  const query = $("#complexityFilter").value.trim().toLowerCase();
  const sort = $("#complexitySort").value;
  const files = complexity.files.filter((item) => item.path.toLowerCase().includes(query));
  if (sort === "loc-asc") files.sort((a, b) => a.loc - b.loc || a.path.localeCompare(b.path));
  else if (sort === "loc-desc") files.sort((a, b) => b.loc - a.loc || a.path.localeCompare(b.path));
  else files.sort((a, b) => a.maintainability - b.maintainability || b.cognitive - a.cognitive).splice(25);
  const rows = files.map((item) => {
    const miClass = item.maintainability < 45 ? "low" : item.maintainability < 70 ? "mid" : "high";
    return `<tr><td class="path-cell">${esc(item.path)}</td><td>${number(item.loc)}</td><td>${Number(item.cyclomatic).toFixed(2)}</td><td>${Number(item.cognitive).toFixed(2)}</td><td><span class="mi-value ${miClass}">${Number(item.maintainability).toFixed(1)}</span></td><td><span class="complexity-bar" aria-label="Maintainability ${Number(item.maintainability).toFixed(1)} out of 100"><i style="width:${Math.max(0, Math.min(100, item.maintainability))}%"></i></span></td></tr>`;
  });
  $("#complexityTable").innerHTML = table(["Module", "LOC", "Cyclomatic avg", "Cognitive avg", "Mean function MI", "Index"], rows, "No matching source files");
}

function renderRuns(data) {
  const { runs, activeRun } = data;
  const passedRuns = runs.filter((run) => run.status === "passed").length;
  const rate = runs.length ? Math.round(passedRuns * 100 / runs.length) : null;
  const latestTotals = testTotals(activeRun || runs.at(-1));
  $("#runHistoryGrid").innerHTML = [
    metricCard("Recorded runs", number(runs.length), "Saved on this machine", "◷"),
    metricCard("Passing runs", `${rate == null ? "—" : rate}%`, `${passedRuns} of ${runs.length} completed`, "✓"),
    metricCard("Latest test pass", latestTotals.known ? number(latestTotals.passed) : "—", "Test cases passed", "+"),
    metricCard("Latest test fail", latestTotals.known ? number(latestTotals.failed) : "—", "Test cases failed", "!"),
  ].join("");
  const rows = runs.map((run) => {
    const totals = testTotals(run);
    const counts = totals.known ? `${number(totals.passed)} passed · ${number(totals.failed)} failed · ${number(totals.skipped)} skipped` : "Counts unavailable";
    const checks = (run.checks || []).map((check) => `${check.suite === "rust" ? "Rust" : check.suite === "dashboard" ? "Dashboard" : "Frontend"}: ${statusText(check.status)}`).join(" · ");
    return `<tr><td>${esc(shortDate(run.finishedAt))}</td><td class="commit-tag">${esc(run.commit || "—")}</td><td><span class="status-pill ${statusClass(run.status)}">${esc(statusText(run.status))}</span></td><td>${esc(counts)}</td><td>${esc(checks)}</td></tr>`;
  });
  $("#runsTable").innerHTML = table(["Finished", "Commit", "Status", "Test totals", "Suites"], rows, activeRun ? "The current run will appear here when it finishes" : "No runs recorded yet");
}

function renderButtons(data) {
  const busy = Boolean(data.activeRun);
  $$('[data-run]').forEach((button) => {
    button.disabled = busy;
    button.textContent = busy ? "Tests running…" : button.dataset.run === "all" ? "Run all automated tests ↗" : button.dataset.run === "rust" ? "Run Rust tests" : "Run frontend";
  });
}

function render(data) {
  snapshot = data;
  $("#lastUpdated").textContent = `Updated ${new Date(data.updatedAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" })}`;
  renderOverview(data);
  renderChanges(data);
  renderTests(data);
  renderComplexity(data);
  renderRuns(data);
  renderButtons(data);
}

async function refresh(showFailure = false) {
  try {
    const response = await fetch("/api/metrics", { cache: "no-store" });
    if (!response.ok) throw new Error(`Dashboard returned ${response.status}`);
    render(await response.json());
  } catch (error) {
    $("#lastUpdated").textContent = "Offline";
    if (showFailure) showToast(`Could not refresh metrics: ${error.message}`);
  }
}

function showToast(message) {
  const toast = $("#toast");
  toast.textContent = message;
  toast.classList.add("visible");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => toast.classList.remove("visible"), 3200);
}

async function startRun(suite) {
  try {
    const response = await fetch("/api/run", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ suite }) });
    const result = await response.json();
    if (!response.ok) throw new Error(result.error || `Request failed (${response.status})`);
    const label = suite === "all"
      ? "Rust, frontend, and Python dashboard tests"
      : suite === "rust"
        ? "Rust tests"
        : suite === "frontend"
          ? "Frontend tests"
          : "Python dashboard tests";
    showToast(`${label} started.`);
    await refresh();
  } catch (error) {
    showToast(error.message);
  }
}

function setView(name) {
  if (!viewNames[name]) return;
  $$(".view").forEach((view) => view.classList.toggle("active", view.id === `view-${name}`));
  $$(".nav-item").forEach((button) => button.classList.toggle("active", button.dataset.view === name));
  $("#crumbTitle").textContent = viewNames[name];
  window.scrollTo({ top: 0, behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "instant" : "smooth" });
}

$(".nav-list").addEventListener("click", (event) => {
  const button = event.target.closest("[data-view]");
  if (button) setView(button.dataset.view);
});
$$("[data-open]").forEach((button) => button.addEventListener("click", () => setView(button.dataset.open)));
$$('[data-run]').forEach((button) => button.addEventListener("click", () => startRun(button.dataset.run)));
$("#refreshButton").addEventListener("click", () => refresh(true));
$("#changeFilter").addEventListener("input", () => snapshot && renderChanges(snapshot));
$("#testScopeFilter").addEventListener("change", () => snapshot && renderTests(snapshot));
$("#testTypeFilter").addEventListener("change", () => snapshot && renderTests(snapshot));
$("#testFilter").addEventListener("input", () => snapshot && renderTests(snapshot));
$("#complexityFilter").addEventListener("input", () => snapshot && renderComplexity(snapshot));
$("#complexitySort").addEventListener("change", () => snapshot && renderComplexity(snapshot));

refresh(true);
setInterval(() => refresh(false), 5000);
