import { createEffect, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { api, previewPhases, previewFailedState, previewHfLockedState, previewInstallingState, previewStartingState, previewState, type AppError, type Phase, type Settings, type State } from "./bridge";
import { previewLogs } from "./mock";

type InlinePanel = "install" | "remove" | "keep" | null;
const mockMode = () => import.meta.env.DEV && !("__TAURI_INTERNALS__" in window);
const workingPhases: Phase[] = ["installing", "starting", "stopping", "removing"];

export default function App() {
  const [state, setState] = createSignal<State>(previewState);
  const [logs, setLogs] = createSignal<string[]>(mockMode() ? [...previewLogs] : []);
  const [panel, setPanel] = createSignal<InlinePanel>(null);
  const [removeText, setRemoveText] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [requestError, setRequestError] = createSignal<AppError | null>(null);
  const [legacySettingsPath, setLegacySettingsPath] = createSignal<string | null>(null);
  const [legacySearchComplete, setLegacySearchComplete] = createSignal(false);
  const [version, setVersion] = createSignal("dev");
  const [followingOutput, setFollowingOutput] = createSignal(true);
  const [clock, setClock] = createSignal(Date.now() / 1000);
  const [lastLogAt, setLastLogAt] = createSignal<number | null>(mockMode() ? Date.now() / 1000 - 80 : null);
  const [infoOpen, setInfoOpen] = createSignal(false);
  let output: HTMLDivElement | undefined;
  let infoTrigger: HTMLButtonElement | undefined;
  let infoCloseButton: HTMLButtonElement | undefined;
  const closeInfo = () => {
    setInfoOpen(false);
    infoTrigger?.focus();
  };

  onMount(() => {
    let unState: (() => void) | undefined;
    let unLog: (() => void) | undefined;
    api.getState().then(next => {
      if (workingPhases.includes(next.phase)) setLastLogAt(null);
      setState(next);
    }).catch(error => setRequestError(toAppError(error)));
    api.onState(next => {
      if (workingPhases.includes(next.phase) && !workingPhases.includes(state().phase)) setLastLogAt(null);
      setState(next);
      setRequestError(null);
      if (workingPhases.includes(next.phase)) setPanel(null);
    }).then(unlisten => unState = unlisten);
    api.onLog(line => {
      setLastLogAt(Date.now() / 1000);
      setLogs(lines => line.startsWith("$ ") && lines.length > 0 && lines[lines.length - 1] !== ""
        ? [...lines, "", line]
        : [...lines, line]);
    }).then(unlisten => unLog = unlisten);
    if ("__TAURI_INTERNALS__" in window) {
      import("@tauri-apps/api/app").then(({ getVersion }) => getVersion().then(setVersion)).catch(() => {});
    }
    onCleanup(() => { unState?.(); unLog?.(); });
  });

  createEffect(() => {
    logs();
    if (output && followingOutput()) output.scrollTop = output.scrollHeight;
  });

  const syncOutputPosition = () => {
    if (!output) return;
    setFollowingOutput(output.scrollHeight - output.scrollTop - output.clientHeight < 24);
  };

  const current = () => state();
  const phase = () => current().phase;
  const working = () => workingPhases.includes(phase());
  createEffect(() => {
    if (!working() && phase() !== "running") return;
    setClock(Date.now() / 1000);
    const timer = window.setInterval(() => setClock(Date.now() / 1000), 1000);
    onCleanup(() => window.clearInterval(timer));
  });
  const [probedStart, setProbedStart] = createSignal(false);
  createEffect(() => {
    const now = clock();
    if (phase() !== "starting" && phase() !== "running") {
      if (probedStart()) setProbedStart(false);
      return;
    }
    const since = lastLogAt() ?? current().started_at;
    if (probedStart() || since == null || now - since <= 20) return;
    setProbedStart(true);
    void (async () => {
      const alive = await api.probeStudio().catch(() => false);
      if (alive) return;
      const stopped = await api.stop().catch(() => null);
      if (stopped) setState(stopped);
      const url = current().studio_url ?? `http://127.0.0.1:${current().settings.port}`;
      setLogs(lines => [...lines, `Warning: Studio is not answering on ${url}. The start failed, so it was stopped. Use Update to repair the installation.`]);
    })();
  });
  createEffect(() => {
    if (!menuOpen()) return;
    const closeOnOutside = (event: PointerEvent) => {
      const target = event.target as HTMLElement | null;
      if (target?.closest(".overflow")) return;
      setMenuOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setMenuOpen(false);
    };
    document.addEventListener("pointerdown", closeOnOutside);
    window.addEventListener("keydown", closeOnEscape);
    onCleanup(() => {
      document.removeEventListener("pointerdown", closeOnOutside);
      window.removeEventListener("keydown", closeOnEscape);
    });
  });
  createEffect(() => {
    if (!infoOpen()) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") closeInfo();
    };
    window.addEventListener("keydown", closeOnEscape);
    infoCloseButton?.focus();
    onCleanup(() => window.removeEventListener("keydown", closeOnEscape));
  });

  const mockPhase = (next: string) => {
    if (next === "hf-locked") {
      setState(previewHfLockedState);
      return;
    }
    setState(previewState);
    setLogs([...previewLogs]);
    if (next === "installing-preview" || next === "starting-preview") {
      const installing = next === "installing-preview";
      setState(installing ? previewInstallingState : previewStartingState);
      setLastLogAt(Date.now() / 1000 - 80);
      setLogs(installing ? [...previewLogs.slice(0, 7), "mise install", "  installing cmake@latest", "  installing python@3.13.9"] : ["Starting Studio", "Checking managed environment"]);
      setPanel(null);
      return;
    }
    if (!previewPhases.includes(next as Phase)) return;
    if (next === "installing" || next === "starting") {
      setState(next === "installing" ? previewInstallingState : previewStartingState);
      setLastLogAt(Date.now() / 1000 - 80);
      setPanel(null);
      return;
    }
    if (next === "failed") {
      setState(previewFailedState);
      setLastLogAt(null);
      setPanel(null);
      return;
    }
    setState(value => ({
      ...value,
      phase: next as Phase,
      step: null,
      started_at: null,
      step_started_at: null,
      elapsed_secs: null,
      studio_url: next === "running" ? `http://localhost:${value.settings.port}` : null,
      last_error: next === "failed" ? { code: "internal", message: "Preview error. No launcher operation ran." } : null,
    }));
    setPanel(null);
  };

  const perform = async (action: () => Promise<State | void>) => {
    setBusy(true);
    setRequestError(null);
    try {
      const result = await action();
      if (result) setState(result);
    } catch (error) {
      setRequestError(toAppError(error));
    } finally {
      setBusy(false);
    }
  };
  const searchLegacySettings = async () => {
    setBusy(true);
    setRequestError(null);
    try {
      setLegacySettingsPath(await api.findLegacySettings());
      setLegacySearchComplete(true);
    } catch (error) {
      setRequestError(toAppError(error));
    } finally {
      setBusy(false);
    }
  };
  const migrateLegacySettings = async () => {
    setBusy(true);
    setRequestError(null);
    try {
      setState(await api.migrateSettings());
      setLegacySettingsPath(null);
      setLegacySearchComplete(false);
    } catch (error) {
      setRequestError(toAppError(error));
    } finally {
      setBusy(false);
    }
  };

  const guardRunning = () => {
    if (!working()) return false;
    setRequestError({
      code: "operation_in_progress",
      message: "The install folder cannot be changed while an operation is running. Wait for it to finish and try again.",
    });
    return true;
  };

  const changeRoot = async () => {
    if (guardRunning()) return;
    try {
      const parent = await api.pickDirectory("root_parent");
      if (!parent) return;
      const folder = current().settings.folder_name?.trim() || "Unsloth Studio Desktop";
      const root = `${parent.replace(/[\\/]+$/, "")}\\${folder}`;
      setState(await api.saveSettings({ ...current().settings, root }));
      setRequestError(null);
    } catch (error) {
      setRequestError(toAppError(error));
    }
  };

  const changeHfHome = async () => {
    try {
      const hfHome = await api.pickDirectory("hf_home");
      if (!hfHome) return;
      setState(await api.saveSettings({ ...current().settings, hf_home: hfHome }));
      setRequestError(null);
    } catch (error) {
      setRequestError(toAppError(error));
    }
  };

  const revealPath = (target: "root" | "mise_config" | "hf_home") => {
    api.revealPath(target).catch(error => setRequestError(toAppError(error)));
  };

  const windowAction = (kind: "minimize" | "toggle" | "close") => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    import("@tauri-apps/api/window").then(({ getCurrentWindow }) => {
      const win = getCurrentWindow();
      if (kind === "minimize") void win.minimize();
      else if (kind === "toggle") void win.toggleMaximize();
      else void win.close();
    }).catch(() => {});
  };

  const openLink = (url: string) => {
    api.openUrl(url).catch(error => setRequestError(toAppError(error)));
  };

  const [portEditing, setPortEditing] = createSignal(false);
  const [portText, setPortText] = createSignal("");
  const [menuOpen, setMenuOpen] = createSignal(false);
  const [folderText, setFolderText] = createSignal("");
  const [forceMise, setForceMise] = createSignal(false);
  const [menuTab, setMenuTab] = createSignal<"general" | "experimental">("general");
  const editPort = () => {
    setPortText(String(current().settings.port));
    setPortEditing(true);
  };
  const commitPort = async () => {
    const port = Number(portText().trim());
    setPortEditing(false);
    if (!Number.isInteger(port) || port < 1 || port > 65535 || port === current().settings.port) return;
    try {
      setState(await api.saveSettings({ ...current().settings, port }));
      setRequestError(null);
    } catch (error) {
      setRequestError(toAppError(error));
    }
  };

  const saveExperimental = async (moveFolder = true) => {
    if (guardRunning()) return;
    try {
      setState(await api.saveSettings({
        ...current().settings,
        folder_name: folderText().trim(),
        force_local_mise: forceMise(),
      }, moveFolder));
      setRequestError(null);
    } catch (error) {
      setRequestError(toAppError(error));
    }
  };

  // The switch persists on change, so an install started right after it keeps
  // the choice; the folder name still needs its explicit Save.
  const toggleForceMise = async (value: boolean) => {
    setForceMise(value);
    try {
      setState(await api.saveSettings({ ...current().settings, force_local_mise: value }));
      setRequestError(null);
    } catch (error) {
      setForceMise(current().settings.force_local_mise);
      setRequestError(toAppError(error));
    }
  };

  const confirmInstall = () => {
    setPanel(null);
    perform(() => api.install(true));
  };

  const confirmRemove = () => {
    setPanel(null);
    perform(() => api.remove(removeText()));
  };

  const installEnabled = () => !working() && (phase() === "absent" || phase() === "failed" || phase() === "installed");
  const updating = () => phase() === "installed";
  const startEnabled = () => !working() && phase() === "installed";
  const stopEnabled = () => !working() && (phase() === "starting" || phase() === "running");
  const openEnabled = () => !working() && (phase() === "installed" || phase() === "running");
  const removeEnabled = () => !working() && (phase() === "installed" || phase() === "failed");
  const totalElapsed = () => working()
    ? current().started_at == null ? null : Math.max(0, clock() - current().started_at)
    : current().elapsed_secs ?? null;
  const stalledFor = () => {
    if (!working()) return null;
    const lastOutput = lastLogAt() ?? current().started_at;
    if (lastOutput == null) return null;
    const seconds = clock() - lastOutput;
    return seconds > 20 ? seconds : null;
  };

  return <div class="app">
    <header class="titlebar" data-tauri-drag-region>
      <div class="titlebar-left" data-tauri-drag-region>
        <span class="wordmark" data-tauri-drag-region>Unsloth Studio Desktop</span>
        <div class="overflow">
          <button class="info-button" title="Settings" aria-label="Settings" aria-haspopup="menu" aria-expanded={menuOpen()} onClick={() => { setPortText(String(current().settings.port)); setFolderText(current().settings.folder_name ?? ""); setForceMise(current().settings.force_local_mise ?? false); setMenuTab("general"); setMenuOpen(!menuOpen()); }}>
            <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2.5 5.5h11M2.5 10.5h11" /><circle cx="6" cy="5.5" r="1.6" /><circle cx="10" cy="10.5" r="1.6" /></svg>
          </button>
          <Show when={menuOpen()}>
            <div class="overflow-menu" role="menu">
              <div class="overflow-tabs" role="tablist">
                <button class={menuTab() === "general" ? "overflow-tab active" : "overflow-tab"} role="tab" aria-selected={menuTab() === "general"} onClick={() => setMenuTab("general")}>General</button>
                <button class={menuTab() === "experimental" ? "overflow-tab active" : "overflow-tab"} role="tab" aria-selected={menuTab() === "experimental"} onClick={() => setMenuTab("experimental")}>Experimental</button>
              </div>
              <Show when={menuTab() === "general"}>
                <div class="overflow-row overflow-row-stack">
                  <div class="overflow-text">
                    <span class="overflow-title">Install folder</span>
                    <p>Choose where Unsloth Studio and its managed tools are installed. An existing installation in the selected folder is detected automatically; use Update to move it forward instead of reinstalling.</p>
                    <p class="overflow-path">{current().settings.root}</p>
                  </div>
                  <div class="overflow-port">
                    <button class="text-button menu-action" disabled={busy()} onClick={() => { setMenuOpen(false); void changeRoot(); }}>
                      <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m10.8 2.7 2.5 2.5M3 13l2.6-.5 7.8-7.8a1.8 1.8 0 0 0-2.5-2.5L3.1 10z" /></svg>
                      <span>Change</span>
                    </button>
                  </div>
                </div>
                <div class="overflow-row overflow-row-stack">
                  <div class="overflow-text">
                    <span class="overflow-title">Settings file</span>
                    <p>This file stores your chosen install folder and options. In portable mode, it lives next to the executable.</p>
                  </div>
                  <p class="overflow-path">{current().settings_path}</p>
                </div>
                <div class="overflow-row overflow-row-stack">
                  <div class="overflow-text">
                    <button class="text-button folder-keep-link" disabled={busy()} onClick={() => void searchLegacySettings()}>Search the default location</button>
                    <Show when={legacySearchComplete() && !legacySettingsPath()}>
                      <p>No settings file was found in the default location.</p>
                    </Show>
                    <Show when={legacySettingsPath()}>
                      <p class="overflow-path">{legacySettingsPath()}</p>
                      <div class="overflow-port">
                        <button class="text-button menu-action" disabled={busy()} onClick={() => void migrateLegacySettings()}>Move settings here</button>
                      </div>
                      <p>The file is moved, not copied.</p>
                    </Show>
                  </div>
                </div>
                <div class="overflow-row overflow-row-stack">
                  <div class="overflow-text">
                    <span class="overflow-title">Studio port</span>
                    <p>Local port used by the Studio web interface. Change it only when another program already uses the current port.</p>
                  </div>
                  <div class="overflow-port">
                    <input class="text-input" type="text" inputmode="numeric" value={portText()} onInput={event => setPortText(event.currentTarget.value)} />
                    <button class="text-button menu-action" disabled={busy() || working()} onClick={() => void commitPort()}>
                      <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3.5 2.5h7l2 2v9h-9z" /><path d="M5.5 2.5v3.5h5V2.5M5.5 13v-3.5h5V13" /></svg>
                      <span>Save</span>
                    </button>
                  </div>
                </div>
                <div class="overflow-row overflow-row-stack">
                  <div class="overflow-text">
                    <span class="overflow-title">Uninstall Studio</span>
                    <p>Deletes the installed Studio, its virtual environment, and the managed mise tools from the install folder. Keeps mise.toml, user data, and the Hugging Face cache.</p>
                  </div>
                  <div class="overflow-port">
                    <button class="text-button menu-action overflow-danger" disabled={!removeEnabled() || busy()} onClick={() => { setMenuOpen(false); setRemoveText(""); setPanel("remove"); }}>
                      <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2.8 4.2h10.4M6 4.2V2.6h4v1.6M4.2 4.2l.7 9.2h6.2l.7-9.2M6.6 6.9v3.9M9.4 6.9v3.9" /></svg>
                      <span>Uninstall</span>
                    </button>
                  </div>
                </div>
              </Show>
              <Show when={menuTab() === "experimental"}>
                <div class="overflow-text">
                  <p>Unstable options. Leave them off unless you know you need them.</p>
                </div>
                <label class="overflow-check">
                  <input type="checkbox" role="switch" checked={forceMise()} disabled={busy() || working()} onChange={event => void toggleForceMise(event.currentTarget.checked)} />
                  <span class="toggle" aria-hidden="true" />
                  <span>Always use a launcher-managed mise</span>
                </label>
                <div class="overflow-row overflow-row-stack">
                  <div class="overflow-text">
                    <span class="overflow-title">Install folder name</span>
                    <p>Renames the product folder inside its current parent folder. Leave it empty to use the default name.</p>
                    <p class="overflow-path">{current().settings.root}</p>
                  </div>
                  <div class="overflow-port">
                    <input class="text-input" type="text" placeholder="Unsloth Studio Desktop" value={folderText()} disabled={busy()} onInput={event => setFolderText(event.currentTarget.value)} />
                    <button class="text-button menu-action" disabled={busy()} onClick={() => void saveExperimental()}>
                      <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3.5 2.5h7l2 2v9h-9z" /><path d="M5.5 2.5v3.5h5V2.5M5.5 13v-3.5h5V13" /></svg>
                      <span>Save</span>
                    </button>
                  </div>
                  <button class="text-button folder-keep-link" disabled={busy()} onClick={() => { setMenuOpen(false); setPanel("keep"); }}>
                    Keep the old folder and install somewhere else?
                  </button>
                </div>
              </Show>
            </div>
          </Show>
        </div>
        <button class="info-button" ref={infoTrigger} aria-label="About this launcher" aria-haspopup="dialog" aria-expanded={infoOpen()} onClick={() => setInfoOpen(true)}>
          <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6.25" /><path d="M8 7.2v3.6M8 4.9h.01" /></svg>
        </button>
      </div>
      <div class="window-controls">
        <button class="window-button" aria-label="Minimize" onClick={() => windowAction("minimize")}><svg viewBox="0 0 10 10" aria-hidden="true"><path d="M1 5h8" /></svg></button>
        <button class="window-button" aria-label="Maximize" onClick={() => windowAction("toggle")}><svg viewBox="0 0 10 10" aria-hidden="true"><rect x="1.4" y="1.4" width="7.2" height="7.2" rx="1" /></svg></button>
        <button class="window-button window-close" aria-label="Close" onClick={() => windowAction("close")}><svg viewBox="0 0 10 10" aria-hidden="true"><path d="m1.8 1.8 6.4 6.4M8.2 1.8 1.8 8.2" /></svg></button>
      </div>
    </header>
    <div class="screen">
    <Show when={mockMode()}>
        <label class="phase-picker">PREVIEW PHASE
          <select value={current().hf_env_var ? "hf-locked" : phase()} onChange={event => mockPhase(event.currentTarget.value)}>
            <For each={previewPhases}>{value => <option value={value}>{value}</option>}</For>
            <option value="installing-preview">Installing · 4/6</option>
            <option value="starting-preview">Starting · 1/2</option>
            <option value="hf-locked">HF folder locked</option>
          </select>
        </label>
      </Show>
    <Show when={infoOpen()}>
      <div class="info-backdrop" onClick={closeInfo}>
        <section class="info-dialog" role="dialog" aria-modal="true" aria-labelledby="info-title" onClick={event => event.stopPropagation()}>
          <header class="info-header">
            <h2 id="info-title">About this launcher</h2>
            <button class="info-close" ref={infoCloseButton} aria-label="Close information" onClick={closeInfo}>×</button>
          </header>
          <section class="info-section">
            <h3>Install folder</h3>
            <p>The launcher checks the selected install folder. If an active installation exists there, the Install action is disabled. To reinstall, empty the install folder; otherwise use Update to move an existing installation forward.</p>
          </section>
          <section class="info-section">
            <h3>Dependencies</h3>
            <p>Dependencies are installed with <a href="#" onClick={event => { event.preventDefault(); openLink("https://github.com/jdx/mise"); }}>jdx/mise</a> locally into the install folder. The launcher never touches the machine's global Python, Bun, or Node versions. Python packages are installed into a local environment instead of the global package table for your Python version.</p>
          </section>
          <section class="info-section">
            <h3>Purpose</h3>
            <p>This launcher gets Unsloth Studio working in one or two clicks without leaving managed files outside its folder. Using Unsloth does not require matching global tool versions, and the product stays independent from the rest of the machine.</p>
          </section>
          <section class="info-section">
            <h3>Made by</h3>
            <p><a href="#" onClick={event => { event.preventDefault(); openLink("https://github.com/rozsazoltan"); }}>rozsazoltan</a> · <a href="#" onClick={event => { event.preventDefault(); openLink("https://github.com/rozsazoltan/unsloth-studio-portable"); }}>unsloth-studio-portable</a></p>
          </section>
        </section>
      </div>
    </Show>

    <section class="controls" aria-label="Studio controls">
      <div class="status-row">
      <div class="status-line">
        <span class="status-dot" classList={{ working: working(), good: phase() === "installed" || phase() === "running", failed: phase() === "failed" }} aria-hidden="true" />
        <Show when={working()} fallback={<>
          <span class="phase">{phase()}</span>
        </>}>
          <span class="progress-track" aria-hidden="true"><span class="progress-sweep" /></span>
          <span class="phase">{current().step
            ? <>({current().step!.index}/{current().step!.total}) {current().step!.name}</>
            : phase()}</span>
          <Show when={working() && phase() !== "starting"}>
            <span class="runtime-metrics">
              <span class="runtime-value">{totalElapsed() == null ? "run —" : `run ${formatDuration(totalElapsed()!)}`}</span>
              <span class="stall-warning" classList={{ stalled: stalledFor() != null, waiting: stalledFor() == null }}>
                {stalledFor() == null ? "no output for —" : `no output for ${formatDuration(stalledFor()!)}`}
              </span>
            </span>
          </Show>
        </Show>
      </div>
      <div class="status-actions">
        <button class="root-path" title={current().settings.root} onClick={() => revealPath("root")}>{current().settings.root}</button>
        <button class="text-button change-folder-button" disabled={busy()} onClick={changeRoot}>
          <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m10.8 2.7 2.5 2.5M3 13l2.6-.5 7.8-7.8a1.8 1.8 0 0 0-2.5-2.5L3.1 10z" /></svg>
          <span>Change folder</span>
        </button>
      </div>
      </div>
      <Show when={current().last_error || requestError()}>
        <p class="error-message"><b>{(requestError() ?? current().last_error)?.code}</b><span>{(requestError() ?? current().last_error)?.message}</span></p>
      </Show>

      <Show when={panel() === null}>
        <div class="button-row">
          <button class="action-button button-primary" disabled={!installEnabled() || busy()} onClick={() => setPanel("install")}>{phase() === "absent" ? "Install" : "Update"}</button>
          <Show when={phase() === "installing"}>
            <button class="action-button button-secondary" onClick={() => perform(api.cancel)}>Cancel</button>
          </Show>
          <button class="action-button button-secondary" disabled={!startEnabled() || busy()} onClick={() => perform(api.start)}>Start</button>
          <button class="action-button button-secondary" disabled={!stopEnabled() || busy()} onClick={() => perform(api.stop)}>Stop</button>
          <button class="action-button button-secondary" disabled={!openEnabled() || busy()} onClick={() => perform(api.openStudio)}>Open Studio</button>
        </div>
      </Show>

      <Show when={panel() === "install"}>
        <section class="inline-panel">
          <div>
            <h2>{updating() ? "Update Studio?" : "Install Studio?"}</h2>
            <p>Setup may install Microsoft Visual C++ runtime and CMake, use a PATH-scoped toolchain through mise, and requires WebView2.</p>
          </div>
          <div class="inline-actions">
            <button class="action-button button-secondary" disabled={busy() || working()} onClick={() => setPanel(null)}>Cancel</button>
            <button class="action-button button-primary" disabled={busy() || working()} onClick={confirmInstall}>{updating() ? "Update Studio" : "Install Studio"}</button>
          </div>
        </section>
      </Show>

      <Show when={panel() === "keep"}>
        <section class="inline-panel">
          <div>
            <h2>Keep the old folder?</h2>
            <p>The current folder stays untouched. The launcher switches to the new name, checks it for an installed Studio, and works with that folder from then on.</p>
          </div>
          <div class="inline-actions">
            <button class="action-button button-secondary" disabled={busy() || working()} onClick={() => setPanel(null)}>No</button>
            <button class="action-button button-primary" disabled={busy() || working()} onClick={() => { setPanel(null); void saveExperimental(false); }}>Yes, keep old folder</button>
          </div>
        </section>
      </Show>

      <Show when={panel() === "remove"}>
        <section class="inline-panel remove-panel">
          <div>
            <h2>Uninstall Studio?</h2>
            <p>This deletes the installed Studio, its virtual environment, and the managed mise tools from the install folder. It keeps your mise.toml, user data, and the Hugging Face cache.</p>
          </div>
          <label class="confirm-field">Type REMOVE to confirm
            <input value={removeText()} onInput={event => setRemoveText(event.currentTarget.value)} autocomplete="off" />
          </label>
          <div class="inline-actions">
            <button class="action-button button-secondary" disabled={busy() || working()} onClick={() => setPanel(null)}>Cancel</button>
            <button class="action-button button-danger" disabled={busy() || working() || removeText() !== "REMOVE"} onClick={confirmRemove}>Remove Studio</button>
          </div>
        </section>
      </Show>
    </section>

    <section class="output-section" aria-label="Operation output">
      <div class="output-heading">
        <span>OUTPUT</span>
        <Show when={logs().length > 0}>
          <button class="text-button" onClick={() => { setLogs([]); setFollowingOutput(true); }}>Clear</button>
        </Show>
      </div>
      <div class="output-wrap">
      <div class="output-pane" ref={output} onScroll={syncOutputPosition}>
        <Show when={logs().length > 0} fallback={<div class="empty-output">Operation output will appear here.</div>}>
          <For each={logs()}>{line => {
            const text = line.replace(/\r+$/, "");
            const lower = text.toLowerCase();
            const tone = /(^|[\s:])(?:fatal|error|failed)(?:[\s:]|$)/i.test(text) || /\bstopped\b/i.test(text) ? "error"
              : /^(?:tip|note|hint)\b/i.test(text) || /\b(?:warning|warn)\b/i.test(text) ? "warning"
              : /[✓]|\b(?:installed|complete|completed)\b/i.test(text) ? "success"
              : /^(?:downloading|extracting|verifying|\s*\d+%|\s*[|█])/i.test(text) ? "dim"
              : "neutral";
            return <div class={`output-line ${tone}`} title={lower}>{text || "\u00a0"}</div>;
          }}</For>
        </Show>
      </div>
      <Show when={!followingOutput() && logs().length > 0}>
        <button class="jump-latest" onClick={() => { if (output) output.scrollTop = output.scrollHeight; setFollowingOutput(true); }}>Jump to latest</button>
      </Show>
      </div>
    </section>

    <footer class="footer">
      <div class="metadata-item hf-metadata"><span>HF</span><span class="metadata-value clickable" title={current().settings.hf_home} onClick={() => revealPath("hf_home")}>{current().settings.hf_home}</span><Show when={current().hf_env_var}>{name => <span class="hf-lock-note">(managed by {name()})</span>}</Show><Show when={!current().hf_env_var}>
        <button class="icon-button" aria-label="Change Hugging Face folder" disabled={busy() || working()} onClick={changeHfHome}>
          <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m10.8 2.7 2.5 2.5M3 13l2.6-.5 7.8-7.8a1.8 1.8 0 0 0-2.5-2.5L3.1 10z" /></svg>
        </button>
      </Show></div>
      <div class="metadata-item"><span>PORT</span>
        <Show when={!portEditing()} fallback={<input class="text-input port-input" type="text" inputmode="numeric" value={portText()} onInput={event => setPortText(event.currentTarget.value)} onBlur={commitPort} onKeyDown={event => { if (event.key === "Enter") commitPort(); if (event.key === "Escape") setPortEditing(false); }} />}>
          <span class="metadata-value">{current().settings.port}</span>
          <button class="icon-button" aria-label="Change port" disabled={busy() || working()} onClick={editPort}>
            <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m10.8 2.7 2.5 2.5M3 13l2.6-.5 7.8-7.8a1.8 1.8 0 0 0-2.5-2.5L3.1 10z" /></svg>
          </button>
        </Show>
      </div>
      <div class="metadata-item mise-metadata"><span>MISE</span><span class="metadata-value clickable" title={current().mise_config_path} onClick={() => revealPath("mise_config")}>{current().mise_config_path}</span></div>
      <div class="metadata-item footer-author">
        <a class="footer-link" href="#" onClick={event => { event.preventDefault(); openLink("https://github.com/rozsazoltan"); }}>rozsazoltan</a>
        <a class="footer-link footer-star" href="#" onClick={event => { event.preventDefault(); openLink("https://github.com/rozsazoltan/unsloth-studio-portable"); }}>
          <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m8 1.8 1.9 3.9 4.3.6-3.1 3 .7 4.3L8 11.5 4.2 13.6l.7-4.3-3.1-3 4.3-.6z" /></svg>
          <span>Give it a star</span>
        </a>
      </div>
      <div class="metadata-item version-item"><span class="metadata-value">v{version()}</span></div>
    </footer>
    </div>

  </div>;
}

function toAppError(error: unknown): AppError {
  if (error && typeof error === "object" && "code" in error && "message" in error) {
    return { code: String(error.code), message: String(error.message) };
  }
  return { code: "internal", message: error instanceof Error ? error.message : String(error) };
}

function formatDuration(seconds: number): string {
  const value = Math.max(0, Math.floor(seconds));
  const hours = Math.floor(value / 3600);
  const minutes = Math.floor(value % 3600 / 60);
  const remaining = value % 60;
  if (hours) return `${hours}h ${String(minutes).padStart(2, "0")}m ${String(remaining).padStart(2, "0")}s`;
  if (minutes) return `${minutes}m ${String(remaining).padStart(2, "0")}s`;
  return `${remaining}s`;
}
