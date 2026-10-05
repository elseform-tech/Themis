# Themis UI rules

Polish the existing workspace; preserve its conversation and review workflows.

- Keep a permanent icon-only utility rail. Each icon has one clear tooltip and accessible name. The expandable dock contains Themis, New chat, projects and threads. Animate its width with the canvas so collapse/expand moves smoothly; reduced motion uses an immediate toggle.
- Pin the protected Themis project at `~/ThemisOS/Projects/Themis`. All new projects live under `~/ThemisOS/Projects`. No setting or creation action changes this root. Renaming other projects changes their display name only; folders and history stay in place. Existing folders are never moved automatically.
- Edit thread changes only its name. The composer selects the model between runs. Each completed response displays its captured model, including after restart or a later switch. Never infer old response models from the current selection; use “Model not recorded” for missing historical metadata.
- Use dark mode only. Offer 17 coordinated templates, including GitHub Dark and Dimmed, plus 12 local font choices and 12–22px text sizes. Preview changes throughout the app; Apply saves, Revert or leaving Settings restores saved choices. Code remains monospace.
- Use unboxed text and icon actions where practical. Keep settings labels short while retaining necessary permission explanations.
- Show selected threads with shimmering text, without a colored side stripe. Use continuous surface layers in every palette: lighter icon/header frame, dark collapsible sidebar, darker canvas. Separate them through their backgrounds, without bright outlines or frame gutters. Appearance swatches and the live surface preview show this same order.
- Overflowing project and thread names start at the beginning, then move right to left on hover or keyboard focus. Preserve their full accessible names. Reduced motion wraps titles and disables motion and shimmer.
- Remove knight artwork from UI surfaces; retain native app icons and favicon. Show the shimmering Themis wordmark in the expandable dock.

Verification belongs in [UI polish checks](ui-polish-verification.md); appearance details are in [Desktop appearance](appearance.md).
