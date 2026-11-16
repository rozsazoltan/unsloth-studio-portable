import type { Phase, State } from "./bridge";

export const previewPhases: Phase[] = [
  "absent", "installing", "installed", "starting", "running", "stopping", "removing", "failed",
];

export const previewState: State = {
  phase: "absent",
  settings: {
    root: "C:\\Users\\you\\Unsloth Studio Desktop",
    hf_home: "C:\\Users\\you\\Unsloth Studio Desktop\\huggingface",
    port: 3000,
    folder_name: "Unsloth Studio Desktop",
    force_local_mise: false,
  },
  mise_config_path: "C:\\Users\\you\\Unsloth Studio Desktop\\mise.toml",
  settings_path: "C:\\Users\\you\\AppData\\Local\\Unsloth Studio Desktop\\launcher-settings.json",
  studio_url: null,
  last_error: null,
};

const previewNow = Math.floor(Date.now() / 1000);
export const previewInstallingState: State = {
  ...previewState,
  phase: "installing",
  started_at: previewNow - 14 * 60 - 12,
  step_started_at: previewNow - 3 * 60 - 12,
  step: { index: 4, total: 6, name: "Installing tools" },
};
export const previewStartingState: State = {
  ...previewState,
  phase: "starting",
  started_at: previewNow - 42,
  step_started_at: previewNow - 26,
  step: { index: 1, total: 2, name: "Starting Studio" },
};
export const previewFailedState: State = {
  ...previewState,
  phase: "failed",
  elapsed_secs: 247,
  last_error: { code: "internal", message: "Preview error. No launcher operation ran." },
};

export const previewHfLockedState: State = {
  ...previewState,
  hf_env_var: "HF_HOME",
  settings: { ...previewState.settings, hf_home: "D:\\Models\\huggingface" },
};

export const previewLogs = [
  "mise 2026.10.4",
  "",
  "downloading python@3.13.9",
  "  14% |█████                         |  38.2 MiB / 268.0 MiB",
  "extracting python@3.13.9",
  "verifying python@3.13.9",
  "✓ python@3.13.9",
  "downloading uv@0.13.1",
  "✓ uv@0.13.1 installed",
  "",
  "mise install complete",
];
