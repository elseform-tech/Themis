import { PromptInput } from "../components/PromptInput";
import { usePromptSkills } from "../lib/usePromptSkills";
import { useEffect, useState } from "react";
import opencodeLogoDark from "../assets/opencode-logo-dark.svg";
import opencodeLogoLight from "../assets/opencode-logo-light.svg";
import {
  Badge,
  Button,
  Dialog,
  EmptyState,
  Input,
} from "../components";
import {
  createAutomation,
  createProject,
  deleteAutomation,
  listGoModels,
  runAutomationNow,
  setAutomationEnabled,
  updateAutomation,
} from "../lib/tauri";
import type { GoModel } from "../lib/types";
import { describeError, toast, useApp } from "../state/store";
import {
  automationToForm,
  emptyAutomationForm,
  saveAutomation,
  toAutomationInput,
  validateAutomationForm,
  type AutomationFormState,
} from "./automationForm";
import "./Automations.css";


function formatTime(iso: string | null): string {
  if (iso === null) return "never";
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

export function Automations() {
  const { state, dispatch } = useApp();
  const [dialog, setDialog] = useState<
    | { mode: "closed" }
    | { mode: "create"; form: AutomationFormState }
    | { mode: "edit"; automationId: string; form: AutomationFormState }
  >({ mode: "closed" });
  const [deleting, setDeleting] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [deleteBusy, setDeleteBusy] = useState(false);
  const [rowBusy, setRowBusy] = useState<string | null>(null);
  const [newProjectName, setNewProjectName] = useState<string | null>(null);
  const [creatingProject, setCreatingProject] = useState(false);
  const [projectError, setProjectError] = useState("");
  const [models, setModels] = useState<GoModel[]>([]);
  const [modelsLoading, setModelsLoading] = useState(false);
  const [modelsError, setModelsError] = useState("");
  const [modelRefresh, setModelRefresh] = useState(0);
  const targetMode = dialog.mode === "closed" ? "closed" : dialog.form.targetMode;
  const threadChoices = state.projects.flatMap(project =>
    (state.threadsByProject[project.root] ?? []).map(thread => ({ project, thread })),
  );
  const selectedThread = dialog.mode === "closed" ? undefined : threadChoices.find(choice => choice.thread.id === dialog.form.targetThreadId);
  const selectedModel = dialog.mode === "closed" ? undefined : models.find(model => model.id === dialog.form.model);
  const effortLevels = selectedModel?.effort_levels ?? [];
  const { skills: promptSkills, error: promptSkillsError } = usePromptSkills(dialog.mode === "closed" ? state.activeProjectRoot : dialog.form.projectRoot);

  useEffect(() => {
    if (targetMode !== "new" || !state.secretStatus.go) return;
    let active = true;
    setModelsLoading(true); setModelsError(""); setModels([]);
    void listGoModels().then(models => {
      if (active) setModels(models);
    }).catch((error: unknown) => {
      if (active) setModelsError(describeError(error));
    }).finally(() => {
      if (active) setModelsLoading(false);
    });
    return () => { active = false; };
  }, [targetMode, state.secretStatus.go, modelRefresh]);

  function openCreate() {
    setSaveError(null);
    setNewProjectName(null);
    setProjectError("");
    setDialog({
      mode: "create",
      form: {
        ...emptyAutomationForm(state.settings.default_provider),
        targetThreadId: state.activeThreadId ?? "",
        projectRoot: state.activeProjectRoot ?? "",
        model: state.settings.default_model,
      },
    });
  }

  function openEdit(automationId: string) {
    const automation = state.automations.find((a) => a.id === automationId);
    if (automation === undefined) return;
    setSaveError(null);
    setNewProjectName(null);
    setProjectError("");
    setDialog({ mode: "edit", automationId, form: automationToForm(automation) });
  }

  function patchForm(patch: Partial<AutomationFormState>) {
    setDialog((prev) =>
      prev.mode === "closed" ? prev : { ...prev, form: { ...prev.form, ...patch } },
    );
  }

  async function makeProject() {
    if (dialog.mode === "closed" || creatingProject || !newProjectName?.trim()) return;
    setCreatingProject(true);
    setProjectError("");
    try {
      const project = await createProject(newProjectName.trim());
      dispatch({ type: "project/opened", project, select: false });
      patchForm({ projectRoot: project.root });
      setNewProjectName(null);
    } catch (error: unknown) {
      setProjectError(describeError(error));
    } finally {
      setCreatingProject(false);
    }
  }


  async function save() {
    if (dialog.mode === "closed" || saving) return;
    if (dialog.form.targetMode === "new") {
      const selected = selectedModel;
      if (!selected) { setSaveError("Select an available OpenCode Go model."); return; }
      if (dialog.form.effort && !selected.effort_levels.includes(dialog.form.effort)) {
        setSaveError("Select an effort level available for this model."); return;
      }
    }
    const invalid = validateAutomationForm(dialog.form);
    if (invalid !== null) {
      setSaveError(invalid);
      return;
    }
    if (dialog.form.targetMode === "continue") {
      if (!selectedThread) { setSaveError("Select an existing thread to continue."); return; }
    } else {
      if (!state.projects.some(project => project.root === dialog.form.projectRoot)) {
        setSaveError("Select a project for the automation thread.");
        return;
      }
    }
    setSaving(true);
    setSaveError(null);
    const result = await saveAutomation(
      { createAutomation, updateAutomation },
      dispatch,
      dialog.mode === "edit" ? dialog.automationId : null,
      toAutomationInput(dialog.form),
    );
    setSaving(false);
    if (result.ok) {
      setDialog({ mode: "closed" });
    } else {
      setSaveError(result.error);
    }
  }

  async function confirmDelete() {
    if (deleting === null || deleteBusy) return;
    const automationId = deleting;
    setDeleteBusy(true);
    try {
      await deleteAutomation(automationId);
      dispatch({ type: "automation/removed", automationId });
      toast(dispatch, "Automation deleted", "success");
      setDeleting(null);
    } catch (error: unknown) {
      toast(dispatch, `Delete failed: ${describeError(error)}`, "danger");
    } finally {
      setDeleteBusy(false);
    }
  }

  async function toggleEnabled(automationId: string, enabled: boolean) {
    if (rowBusy !== null) return;
    setRowBusy(automationId);
    try {
      const automation = await setAutomationEnabled(automationId, enabled);
      dispatch({ type: "automation/updated", automation });
      toast(
        dispatch,
        enabled ? `Enabled ${automation.name}` : `Disabled ${automation.name}`,
        "success",
      );
    } catch (error: unknown) {
      toast(dispatch, `Toggle failed: ${describeError(error)}`, "danger");
    } finally {
      setRowBusy(null);
    }
  }

  async function runNow(automationId: string) {
    if (rowBusy !== null) return;
    setRowBusy(automationId);
    try {
      const run = await runAutomationNow(automationId);
      toast(dispatch, `Run started (thread ${run.thread_id})`, "success");
    } catch (error: unknown) {
      toast(dispatch, `Run failed: ${describeError(error)}`, "danger");
    } finally {
      setRowBusy(null);
    }
  }

  const deleteTarget =
    state.automations.find((a) => a.id === deleting) ?? null;

  return (
    <div className="themis-automations">
      <div className="themis-automations-head">
        <h2 className="themis-automations-title">Automations</h2>
        <Button variant="primary" size="small" onClick={openCreate}>
          + New automation
        </Button>
      </div>
      {!state.settings.automations_enabled && (
        <p className="themis-automations-disabled-note" role="note">
          The scheduler is disabled in Settings — automations will not run on
          their interval (Run now still works).
        </p>
      )}

      {state.automations.length === 0 ? (
        <EmptyState
          title="No automations"
          hint="Create an automation to run a scheduled agent in a project."
        />
      ) : (
        <ul className="themis-automations-list">
          {state.automations.map((automation) => {
            const target = threadChoices.find(choice => choice.thread.id === automation.target_thread_id);
            const busy = rowBusy === automation.id;
            return (
              <li key={automation.id} className="themis-automations-card">
                <div className="themis-automations-card-head">
                  <span className="themis-automations-name">
                    {automation.name}
                  </span>
                  {automation.enabled ? (
                    <Badge tone="success">enabled</Badge>
                  ) : (
                    <Badge tone="default">disabled</Badge>
                  )}
                </div>
                <dl className="themis-automations-meta">
                  <div>
                    <dt>Run in</dt>
                    <dd>{automation.target_thread_id ? `Continue ${target?.thread.title ?? "missing thread"}` : "New thread each run"}</dd>
                  </div>
                  <div>
                    <dt>Project</dt>
                    <dd title={automation.project_root}>
                      {automation.project_root}
                    </dd>
                  </div>
                  <div>
                    <dt>Provider</dt>
                    <dd>
                      OpenCode Go ·{" "}
                      {automation.target_thread_id ? target?.thread.model ?? "unavailable" : automation.model || "default"}
                      {(automation.target_thread_id ? target?.thread.reasoning_effort : automation.reasoning_effort) ? ` · ${automation.target_thread_id ? target?.thread.reasoning_effort : automation.reasoning_effort}` : ""}
                    </dd>
                  </div>
                  <div>
                    <dt>Every</dt>
                    <dd>{automation.interval_mins}m</dd>
                  </div>

                  <div>
                    <dt>Last run</dt>
                    <dd>{formatTime(automation.last_run_at)}</dd>
                  </div>
                  <div>
                    <dt>Next run</dt>
                    <dd>{formatTime(automation.next_run_at)}</dd>
                  </div>
                  <div>
                    <dt>Runs</dt>
                    <dd>{automation.run_count}</dd>
                  </div>
                </dl>
                <div className="themis-automations-card-actions">
                  <label className="themis-automations-toggle">
                    <input
                      type="checkbox"
                      checked={automation.enabled}
                      disabled={busy}
                      onChange={(event) =>
                        void toggleEnabled(automation.id, event.target.checked)
                      }
                    />
                    Enabled
                  </label>
                  <Button
                    variant="ghost"
                    size="small"
                    disabled={busy}
                    onClick={() => void runNow(automation.id)}
                  >
                    {busy ? "…" : "Run now"}
                  </Button>
                  <Button
                    variant="ghost"
                    size="small"
                    disabled={busy}
                    onClick={() => openEdit(automation.id)}
                  >
                    Edit
                  </Button>
                  <Button
                    variant="ghost"
                    size="small"
                    disabled={busy}
                    onClick={() => setDeleting(automation.id)}
                  >
                    Delete
                  </Button>
                </div>
              </li>
            );
          })}
        </ul>
      )}

      <Dialog
        open={dialog.mode !== "closed"}
        title={dialog.mode === "edit" ? "Edit automation" : "New automation"}
        onClose={() => {
          if (!saving && !creatingProject) setDialog({ mode: "closed" });
        }}
      >
        {dialog.mode !== "closed" && (
          <div className="themis-automations-dialog">
            <Input
              id="themis-automation-name"
              label="Name"
              placeholder="nightly-triage"
              value={dialog.form.name}
              disabled={saving}
              onChange={(event) => patchForm({ name: event.target.value })}
            />
            <label className="themis-automations-field">Run in
              <select aria-label="Run in" value={dialog.form.targetMode} disabled={saving} onChange={event => patchForm({ targetMode: event.target.value as "continue" | "new", targetThreadId: "", projectRoot: "" })}>
                <option value="continue">Continue an existing thread</option>
                <option value="new">Create a new thread each run</option>
              </select>
            </label>
            {dialog.form.targetMode === "continue" ? <>
              <label className="themis-automations-field">Thread to continue
                <select aria-label="Thread to continue" value={selectedThread?.thread.id ?? ""} disabled={saving} onChange={event => {
                  const choice = threadChoices.find(item => item.thread.id === event.target.value);
                  patchForm({ targetThreadId: event.target.value, projectRoot: choice?.project.root ?? "" });
                }}>
                  <option value="" disabled>{dialog.form.targetThreadId ? "Thread unavailable — choose another" : "Choose a thread"}</option>
                  {state.projects.map(project => {
                    const threads = state.threadsByProject[project.root] ?? [];
                    return threads.length > 0 && <optgroup key={project.root} label={project.name}>
                      {threads.map(thread => <option key={thread.id} value={thread.id}>{thread.title}</option>)}
                    </optgroup>;
                  })}
                </select>
              </label>
              <span className="themis-automations-hint">Each heartbeat continues this thread with its current model and effort. Select skills inside the task prompt.</span>
              {selectedThread && <p className="themis-automations-hint">{selectedThread.project.name} · {selectedThread.thread.model} · {selectedThread.thread.reasoning_effort || "Default effort"}</p>}
            </> : <>
            <div className="themis-automations-field">
              <label className="themis-automations-label" htmlFor="themis-automation-project">Project for new threads</label>
              <div className="themis-automations-project-row">
                <select id="themis-automation-project" value={state.projects.some(project => project.root === dialog.form.projectRoot) ? dialog.form.projectRoot : ""} disabled={saving || creatingProject} onChange={event => patchForm({ projectRoot: event.target.value })}>
                  <option value="" disabled>{dialog.form.projectRoot ? "Current project unavailable — choose a project" : "Choose a project"}</option>
                  {state.projects.map(project => <option key={project.root} value={project.root}>{project.name} · {project.root}</option>)}
                </select>
                <Button variant="ghost" size="small" disabled={saving || creatingProject} onClick={() => { setNewProjectName(""); setProjectError(""); }}>New project…</Button>
              </div>
              <span className="themis-automations-hint">Each run starts a thread in the selected project.</span>
              {newProjectName !== null && <div className="themis-automations-project-create">
                <Input id="themis-automation-project-name" label="New project name" value={newProjectName} disabled={creatingProject} onChange={event => setNewProjectName(event.target.value)} />
                <span className="themis-automations-hint">Create in {state.settings.projects_directory}</span>
                <div className="themis-automations-project-actions"><Button variant="ghost" size="small" disabled={creatingProject} onClick={() => { setNewProjectName(null); setProjectError(""); }}>Cancel</Button><Button variant="primary" size="small" disabled={creatingProject || !newProjectName.trim()} onClick={() => void makeProject()}>{creatingProject ? "Creating…" : "Create project"}</Button></div>
                {projectError && <p className="themis-automations-error" role="alert">{projectError}</p>}
              </div>}
            </div>
            <label className="themis-automations-field">Model
              <span className="themis-automations-model-picker"><span className="themis-provider-logo" role="img" aria-label="OpenCode Go"><img className="themis-provider-logo-dark" src={opencodeLogoDark} alt="" /><img className="themis-provider-logo-light" src={opencodeLogoLight} alt="" /></span><select aria-label="Model" value={selectedModel ? dialog.form.model : ""} disabled={saving || modelsLoading || !state.secretStatus.go || models.length === 0} onChange={event => patchForm({ model: event.target.value, effort: "" })}>
                <option value="" disabled>{modelsLoading ? "Loading models…" : !state.secretStatus.go ? "Connect OpenCode Go to load models" : models.length === 0 ? "No models available" : selectedModel ? "Choose a model" : `Current model unavailable: ${dialog.form.model || "default"}`}</option>
                {models.map(model => <option key={model.id} value={model.id}>{model.id}</option>)}
              </select></span>
            </label>
            <div className="themis-automations-field">
              <label htmlFor="themis-automation-effort">Reasoning effort · {dialog.form.effort || "Default"}</label>
              <input id="themis-automation-effort" aria-label="Reasoning effort" type="range" min="0" max={Math.max(1, effortLevels.length)} step="1" value={Math.max(0, ["", ...effortLevels].indexOf(dialog.form.effort))} disabled={saving || modelsLoading || !state.secretStatus.go || effortLevels.length === 0} aria-valuetext={dialog.form.effort || "Default"} onChange={event => patchForm({ effort: ["", ...effortLevels][Number(event.target.value)] ?? "" })} />
              <span className="themis-automations-hint">Default → {effortLevels[effortLevels.length - 1] ?? "No catalog levels"}</span>
            </div>
            {modelsError && <p className="themis-automations-error" role="alert">{modelsError}</p>}
            <Button variant="ghost" size="small" disabled={saving || modelsLoading || !state.secretStatus.go} onClick={() => setModelRefresh(value => value + 1)}>Refresh models</Button>

            </>}
            <Input
              id="themis-automation-interval"
              label="Interval (minutes, ≥ 1)"
              type="number"
              min={1}
              value={dialog.form.intervalMinsRaw}
              disabled={saving}
              onChange={(event) =>
                patchForm({ intervalMinsRaw: event.target.value })
              }
            />
            <div className="themis-automations-field">
              <label
                className="themis-automations-label"
                htmlFor="themis-automation-task"
              >
                Task
              </label>
              <PromptInput id="themis-automation-task" label="Task" value={dialog.form.task} skills={promptSkills} disabled={saving} placeholder="What the scheduled run should do… / for skills" onChange={task => patchForm({ task })} />
              {promptSkillsError && <p role="alert">{promptSkillsError}</p>}
            </div>
            <label className="themis-automations-check">
              <input
                type="checkbox"
                checked={dialog.form.enabled}
                disabled={saving}
                onChange={(event) =>
                  patchForm({ enabled: event.target.checked })
                }
              />
              Enabled
            </label>

            {saveError !== null && (
              <p className="themis-automations-error" role="alert">
                {saveError}
              </p>
            )}

            <div className="themis-automations-dialog-actions">
              <Button
                variant="ghost"
                disabled={saving || creatingProject}
                onClick={() => setDialog({ mode: "closed" })}
              >
                Cancel
              </Button>
              <Button
                variant="primary"
                disabled={saving || creatingProject || newProjectName !== null}
                onClick={() => void save()}
              >
                {saving ? "Saving…" : "Save"}
              </Button>
            </div>
          </div>
        )}
      </Dialog>

      <Dialog
        open={deleteTarget !== null}
        title="Delete automation?"
        onClose={() => {
          if (!deleteBusy) setDeleting(null);
        }}
      >
        <div className="themis-automations-dialog">
          <p>
            This permanently deletes{" "}
            <strong>{deleteTarget?.name ?? ""}</strong>. Past runs and review
            items are kept.
          </p>
          <div className="themis-automations-dialog-actions">
            <Button
              variant="ghost"
              disabled={deleteBusy}
              onClick={() => setDeleting(null)}
            >
              Cancel
            </Button>
            <Button
              variant="danger"
              disabled={deleteBusy}
              onClick={() => void confirmDelete()}
            >
              {deleteBusy ? "Deleting…" : "Delete automation"}
            </Button>
          </div>
        </div>
      </Dialog>
    </div>
  );
}
