import { useState } from "react";
import {
  Badge,
  Button,
  Dialog,
  Dropdown,
  EmptyState,
  Input,
} from "../components";
import type { ProviderKind } from "../lib/types";
import {
  createAutomation,
  deleteAutomation,
  listAutomations,
  runAutomationNow,
  setAutomationEnabled,
  updateAutomation,
} from "../lib/tauri";
import { describeError, toast, useApp } from "../state/store";
import {
  automationToForm,
  emptyAutomationForm,
  saveAutomation,
  toAutomationInput,
  validateAutomationForm,
  type AutomationFormState,
} from "./automationForm";
import { pickProjectDirectory } from "./projectPick";
import "./Automations.css";

const PROVIDERS: ProviderKind[] = ["go", "openai", "anthropic", "ollama", "custom"];

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
  const [browsing, setBrowsing] = useState(false);

  function openCreate() {
    setSaveError(null);
    setDialog({
      mode: "create",
      form: emptyAutomationForm(state.settings.default_provider),
    });
  }

  function openEdit(automationId: string) {
    const automation = state.automations.find((a) => a.id === automationId);
    if (automation === undefined) return;
    setSaveError(null);
    setDialog({ mode: "edit", automationId, form: automationToForm(automation) });
  }

  function patchForm(patch: Partial<AutomationFormState>) {
    setDialog((prev) =>
      prev.mode === "closed" ? prev : { ...prev, form: { ...prev.form, ...patch } },
    );
  }

  async function browse() {
    if (dialog.mode === "closed" || browsing) return;
    setBrowsing(true);
    try {
      const picked = await pickProjectDirectory();
      if (picked.kind === "path") {
        patchForm({ projectRoot: picked.path });
      } else if (picked.kind === "unavailable") {
        toast(dispatch, "Folder picker unavailable — type the path instead", "warning");
      }
    } finally {
      setBrowsing(false);
    }
  }

  function toggleSkill(skillId: string) {
    if (dialog.mode === "closed") return;
    const has = dialog.form.skillIds.includes(skillId);
    patchForm({
      skillIds: has
        ? dialog.form.skillIds.filter((id) => id !== skillId)
        : [...dialog.form.skillIds, skillId],
    });
  }

  async function save() {
    if (dialog.mode === "closed" || saving) return;
    const invalid = validateAutomationForm(dialog.form);
    if (invalid !== null) {
      setSaveError(invalid);
      return;
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
      try {
        const automations = await listAutomations();
        dispatch({ type: "automation/loaded", automations });
      } catch {
        // Non-fatal: the run started; counts refresh on next boot.
      }
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
            const skillNames = automation.skill_ids.map(
              (id) =>
                state.skills.find((s) => s.id === id)?.name ?? id,
            );
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
                    <dt>Project</dt>
                    <dd title={automation.project_root}>
                      {automation.project_root}
                    </dd>
                  </div>
                  <div>
                    <dt>Provider</dt>
                    <dd>
                      {automation.provider} ·{" "}
                      {automation.model === "" ? "default" : automation.model}
                    </dd>
                  </div>
                  <div>
                    <dt>Every</dt>
                    <dd>{automation.interval_mins}m</dd>
                  </div>
                  <div>
                    <dt>Skills</dt>
                    <dd title={skillNames.join(", ")}>
                      {automation.skill_ids.length === 0
                        ? "none"
                        : `${automation.skill_ids.length}: ${skillNames.join(", ")}`}
                    </dd>
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
          if (!saving) setDialog({ mode: "closed" });
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
            <div className="themis-automations-field">
              <div className="themis-automations-root-row">
                <Input
                  id="themis-automation-root"
                  label="Project root"
                  placeholder="/path/to/project"
                  value={dialog.form.projectRoot}
                  disabled={saving}
                  onChange={(event) =>
                    patchForm({ projectRoot: event.target.value })
                  }
                />
                <Button
                  variant="ghost"
                  size="small"
                  disabled={saving || browsing}
                  onClick={() => void browse()}
                >
                  {browsing ? "…" : "Browse…"}
                </Button>
              </div>
            </div>
            <div className="themis-automations-row">
              <Dropdown
                items={PROVIDERS.map((p) => ({ id: p, label: p }))}
                value={dialog.form.provider}
                onSelect={(id) =>
                  patchForm({ provider: id as ProviderKind })
                }
                label="Provider"
                disabled={saving}
              />
              <Input
                id="themis-automation-model"
                label="Model"
                placeholder="default"
                value={dialog.form.model}
                disabled={saving}
                onChange={(event) =>
                  patchForm({ model: event.target.value })
                }
              />
            </div>
            <div className="themis-automations-field">
              <span className="themis-automations-label">Skills</span>
              {state.skills.length === 0 ? (
                <p className="themis-automations-hint">
                  No skills yet — the run will use no skills.
                </p>
              ) : (
                <div className="themis-automations-skills">
                  {state.skills.map((skill) => (
                    <label
                      key={skill.id}
                      className="themis-automations-check"
                    >
                      <input
                        type="checkbox"
                        checked={dialog.form.skillIds.includes(skill.id)}
                        disabled={saving}
                        onChange={() => toggleSkill(skill.id)}
                      />
                      {skill.name}
                    </label>
                  ))}
                </div>
              )}
            </div>
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
              <textarea
                id="themis-automation-task"
                className="themis-automations-textarea"
                placeholder="What the scheduled run should do…"
                rows={4}
                value={dialog.form.task}
                disabled={saving}
                onChange={(event) => patchForm({ task: event.target.value })}
              />
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
                disabled={saving}
                onClick={() => setDialog({ mode: "closed" })}
              >
                Cancel
              </Button>
              <Button
                variant="primary"
                disabled={saving}
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
