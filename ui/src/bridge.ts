export type Phase = "absent" | "installing" | "installed" | "starting" | "running" | "stopping" | "removing" | "failed";
export interface Settings { root: string; hf_home: string; port: number; folder_name: string; force_local_mise: boolean }
export interface AppError { code: string; message: string }
export interface State {
  phase: Phase;
  settings: Settings;
  mise_config_path: string;
  settings_path: string;
  studio_url: string | null;
  last_error: AppError | null;
  hf_env_var?: string | null;
  step?: { index: number; total: number; name: string } | null;
  started_at?: number | null;
  step_started_at?: number | null;
  elapsed_secs?: number | null;
}

const isTauri = () => "__TAURI_INTERNALS__" in window;
async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) throw { code: "launcher_required", message: "Actions require the Windows launcher." };
  const { invoke: call } = await import("@tauri-apps/api/core");
  return call<T>(command, args);
}

export const api = {
  getState: () => isTauri() ? invoke<State>("get_state") : Promise.resolve(previewState),
  saveSettings: (settings: Settings, moveFolder = true) => isTauri() ? invoke<State>("save_settings", { settings, move_folder: moveFolder }) : Promise.resolve({ ...previewState, settings }),
  pickDirectory: (target: "root_parent" | "hf_home") => invoke<string | null>("pick_directory", { target }),
  install: (approveEffects: boolean) => invoke<State>("install", { approve_effects: approveEffects }),
  cancel: () => isTauri() ? invoke<State>("cancel") : Promise.resolve(previewState),
  start: () => invoke<State>("start"),
  stop: () => invoke<State>("stop"),
  remove: (confirm: string) => invoke<State>("remove", { confirm }),
  openStudio: () => invoke<void>("open_studio"),
  revealPath: (target: "root" | "mise_config" | "hf_home") =>
    isTauri() ? invoke<void>("reveal_path", { target }) : Promise.resolve(),
  probeStudio: () => isTauri() ? invoke<boolean>("probe_studio") : Promise.resolve(true),
  openUrl: (url: string) => isTauri() ? invoke<void>("open_url", { url }) : Promise.resolve(),
  exit: () => invoke<void>("exit_app"),
  onState: async (fn: (state: State) => void) => {
    if (!isTauri()) return () => {};
    const { listen } = await import("@tauri-apps/api/event");
    return listen<State>("state", event => fn(event.payload));
  },
  onLog: async (fn: (line: string) => void) => {
    if (!isTauri()) return () => {};
    const { listen } = await import("@tauri-apps/api/event");
    return listen<string>("log", event => fn(event.payload));
  },
};

import { previewState } from "./mock";
export { previewState, previewPhases, previewHfLockedState, previewInstallingState, previewStartingState, previewFailedState } from "./mock";
