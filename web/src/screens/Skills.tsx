import { useState } from "react";
import { Badge, Button, Dialog, EmptyState, Input } from "../components";
import { createSkill, deleteSkill, updateSkill } from "../lib/tauri";
import { describeError, toast, useApp } from "../state/store";
import {
  emptySkillForm,
  saveSkill,
  skillToForm,
  toSkillInput,
  validateSkillForm,
  type SkillFormState,
} from "./skillForm";
import "./Skills.css";

export function Skills() {
  const { state, dispatch } = useApp();
  const [dialog, setDialog] = useState<
    | { mode: "closed" }
    | { mode: "create"; form: SkillFormState }
    | { mode: "edit"; skillId: string; form: SkillFormState }
  >({ mode: "closed" });
  const [deleting, setDeleting] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [deleteBusy, setDeleteBusy] = useState(false);

  function openCreate() {
    setSaveError(null);
    setDialog({ mode: "create", form: emptySkillForm() });
  }

  function openEdit(skillId: string) {
    const skill = state.skills.find((s) => s.id === skillId);
    if (skill === undefined) return;
    setSaveError(null);
    setDialog({ mode: "edit", skillId, form: skillToForm(skill) });
  }

  function patchForm(patch: Partial<SkillFormState>) {
    setDialog((prev) =>
      prev.mode === "closed" ? prev : { ...prev, form: { ...prev.form, ...patch } },
    );
  }

  async function save() {
    if (dialog.mode === "closed" || saving) return;
    const invalid = validateSkillForm(dialog.form);
    if (invalid !== null) {
      setSaveError(invalid);
      return;
    }
    setSaving(true);
    setSaveError(null);
    const result = await saveSkill(
      { createSkill, updateSkill },
      dispatch,
      dialog.mode === "edit" ? dialog.skillId : null,
      toSkillInput(dialog.form),
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
    const skillId = deleting;
    setDeleteBusy(true);
    try {
      await deleteSkill(skillId);
      dispatch({ type: "skill/removed", skillId });
      toast(dispatch, "Skill deleted", "success");
      setDeleting(null);
    } catch (error: unknown) {
      toast(dispatch, `Delete failed: ${describeError(error)}`, "danger");
    } finally {
      setDeleteBusy(false);
    }
  }

  const deleteTarget = state.skills.find((s) => s.id === deleting) ?? null;

  return (
    <div className="themis-skills">
      <div className="themis-skills-head">
        <h2 className="themis-skills-title">Skills</h2>
        <Button variant="primary" size="small" onClick={openCreate}>
          + New skill
        </Button>
      </div>

      {state.skills.length === 0 ? (
        <EmptyState
          title="No skills"
          hint="Create a skill to give threads reusable instructions and scripts."
        />
      ) : (
        <table className="themis-skills-table">
          <thead>
            <tr>
              <th scope="col">Name</th>
              <th scope="col">Description</th>
              <th scope="col">Tools</th>
              <th scope="col">Scripts</th>
              <th scope="col">
                <span className="themis-skills-visually-hidden">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {state.skills.map((skill) => (
              <tr key={skill.id}>
                <td className="themis-skills-name">{skill.name}</td>
                <td className="themis-skills-desc" title={skill.description}>
                  {skill.description === "" ? "—" : skill.description}
                </td>
                <td>
                  {skill.allowed_tools.length === 0 ? (
                    <Badge tone="info">all</Badge>
                  ) : (
                    <span title={skill.allowed_tools.join(", ")}>
                      {skill.allowed_tools.length}
                    </span>
                  )}
                </td>
                <td>{skill.scripts.length}</td>
                <td className="themis-skills-actions">
                  <Button
                    variant="ghost"
                    size="small"
                    onClick={() => openEdit(skill.id)}
                  >
                    Edit
                  </Button>
                  <Button
                    variant="ghost"
                    size="small"
                    onClick={() => setDeleting(skill.id)}
                  >
                    Delete
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <Dialog
        open={dialog.mode !== "closed"}
        title={dialog.mode === "edit" ? "Edit skill" : "New skill"}
        onClose={() => {
          if (!saving) setDialog({ mode: "closed" });
        }}
      >
        {dialog.mode !== "closed" && (
          <div className="themis-skills-dialog">
            <Input
              id="themis-skill-name"
              label="Name"
              placeholder="code-review"
              value={dialog.form.name}
              disabled={saving}
              onChange={(event) =>
                patchForm({ name: event.target.value })
              }
            />
            <Input
              id="themis-skill-description"
              label="Description"
              placeholder="What this skill is for"
              value={dialog.form.description}
              disabled={saving}
              onChange={(event) =>
                patchForm({ description: event.target.value })
              }
            />
            <div className="themis-skills-field">
              <label
                className="themis-skills-label"
                htmlFor="themis-skill-instructions"
              >
                Instructions
              </label>
              <textarea
                id="themis-skill-instructions"
                className="themis-skills-textarea"
                placeholder="System instructions injected when the skill is attached…"
                rows={5}
                value={dialog.form.instructions}
                disabled={saving}
                onChange={(event) =>
                  patchForm({ instructions: event.target.value })
                }
              />
            </div>
            <Input
              id="themis-skill-tools"
              label="Allowed tools (comma-separated, blank = all)"
              placeholder="read_file, write_file, …"
              value={dialog.form.allowedToolsRaw}
              disabled={saving}
              onChange={(event) =>
                patchForm({ allowedToolsRaw: event.target.value })
              }
            />
            <p className="themis-skills-hint">
              Tool names are validated by the backend on save — unknown names
              are rejected with an error.
            </p>

            <div className="themis-skills-scripts">
              <div className="themis-skills-scripts-head">
                <span className="themis-skills-label">Scripts</span>
                <Button
                  variant="ghost"
                  size="small"
                  disabled={saving}
                  onClick={() =>
                    patchForm({
                      scripts: [
                        ...dialog.form.scripts,
                        { name: "", content: "" },
                      ],
                    })
                  }
                >
                  + Add script
                </Button>
              </div>
              {dialog.form.scripts.length === 0 ? (
                <p className="themis-skills-hint">No scripts.</p>
              ) : (
                dialog.form.scripts.map((script, index) => (
                  <div key={index} className="themis-skills-script">
                    <div className="themis-skills-script-head">
                      <Input
                        id={`themis-skill-script-name-${index}`}
                        label={`Script #${index + 1} name`}
                        placeholder="helper.sh"
                        value={script.name}
                        disabled={saving}
                        onChange={(event) =>
                          patchForm({
                            scripts: dialog.form.scripts.map((s, i) =>
                              i === index
                                ? { ...s, name: event.target.value }
                                : s,
                            ),
                          })
                        }
                      />
                      <Button
                        variant="ghost"
                        size="small"
                        disabled={saving}
                        onClick={() =>
                          patchForm({
                            scripts: dialog.form.scripts.filter(
                              (_, i) => i !== index,
                            ),
                          })
                        }
                      >
                        Remove
                      </Button>
                    </div>
                    <textarea
                      aria-label={`Script #${index + 1} content`}
                      className="themis-skills-textarea"
                      placeholder="Script content…"
                      rows={4}
                      value={script.content}
                      disabled={saving}
                      onChange={(event) =>
                        patchForm({
                          scripts: dialog.form.scripts.map((s, i) =>
                            i === index
                              ? { ...s, content: event.target.value }
                              : s,
                          ),
                        })
                      }
                    />
                  </div>
                ))
              )}
            </div>

            {saveError !== null && (
              <p className="themis-skills-error" role="alert">
                {saveError}
              </p>
            )}

            <div className="themis-skills-dialog-actions">
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
        title="Delete skill?"
        onClose={() => {
          if (!deleteBusy) setDeleting(null);
        }}
      >
        <div className="themis-skills-dialog">
          <p>
            This permanently deletes{" "}
            <strong>{deleteTarget?.name ?? ""}</strong>. Threads and automations
            referencing it keep the id but lose the instructions.
          </p>
          <div className="themis-skills-dialog-actions">
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
              {deleteBusy ? "Deleting…" : "Delete skill"}
            </Button>
          </div>
        </div>
      </Dialog>
    </div>
  );
}
