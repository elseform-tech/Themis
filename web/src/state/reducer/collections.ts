import type { AppAction, AppState } from "../reducer";

export function reduceCollectionActions(
  state: AppState,
  action: AppAction,
): AppState | null {
  switch (action.type) {
    case "skill/loaded":
      return { ...state, skills: action.skills };
    case "skill/added":
      if (state.skills.some((skill) => skill.id === action.skill.id)) {
        return {
          ...state,
          skills: state.skills.map((skill) =>
            skill.id === action.skill.id ? action.skill : skill,
          ),
        };
      }
      return { ...state, skills: [...state.skills, action.skill] };
    case "skill/updated":
      return {
        ...state,
        skills: state.skills.map((skill) =>
          skill.id === action.skill.id ? action.skill : skill,
        ),
      };
    case "skill/removed":
      return {
        ...state,
        skills: state.skills.filter((skill) => skill.id !== action.skillId),
      };
    case "automation/loaded":
      return { ...state, automations: action.automations };
    case "automation/added":
      if (state.automations.some((automation) => automation.id === action.automation.id)) {
        return {
          ...state,
          automations: state.automations.map((automation) =>
            automation.id === action.automation.id ? action.automation : automation,
          ),
        };
      }
      return { ...state, automations: [...state.automations, action.automation] };
    case "automation/updated":
      return {
        ...state,
        automations: state.automations.map((automation) =>
          automation.id === action.automation.id ? action.automation : automation,
        ),
      };
    case "automation/removed":
      return {
        ...state,
        automations: state.automations.filter(
          (automation) => automation.id !== action.automationId,
        ),
      };
    case "review/loaded":
      return { ...state, reviews: action.reviews };
    case "review/added":
      if (state.reviews.some((review) => review.id === action.review.id)) {
        return {
          ...state,
          reviews: state.reviews.map((review) =>
            review.id === action.review.id ? action.review : review,
          ),
        };
      }
      return { ...state, reviews: [...state.reviews, action.review] };
    case "review/updated":
      return {
        ...state,
        reviews: state.reviews.map((review) =>
          review.id === action.review.id ? action.review : review,
        ),
      };
    case "approval/enqueued":
      if (
        state.approvals.some(
          (approval) => approval.approval_id === action.request.approval_id,
        )
      ) {
        return state;
      }
      return { ...state, approvals: [...state.approvals, action.request] };
    case "approval/dequeued":
      return {
        ...state,
        approvals: state.approvals.filter(
          (approval) => approval.approval_id !== action.approvalId,
        ),
      };
    case "settings/loaded":
    case "settings/patched":
      return { ...state, settings: action.settings, settingsLoaded: true };
    case "secrets/loaded":
      return { ...state, secretStatus: action.status };
    default:
      return null;
  }
}
