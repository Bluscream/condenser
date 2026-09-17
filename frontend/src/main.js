// Uses the globals exposed by `withGlobalTauri` so the frontend needs no bundler
// and no network access (the CSP forbids remote scripts).
const { invoke } = window.__TAURI__.core;
const { open } = window.__TAURI__.dialog;

let games = [];
let selectedId = null;

const $ = (id) => document.getElementById(id);

function toast(msg, isError = false) {
  const el = $("toast");
  el.textContent = msg;
  el.classList.toggle("error", isError);
  el.classList.remove("hidden");
  clearTimeout(toast._t);
  toast._t = setTimeout(() => el.classList.add("hidden"), 4000);
}

function fmtPlaytime(seconds) {
  if (!seconds) return "never played";
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  return h > 0 ? `${h}h ${m}m` : `${m}m`;
}

async function refresh() {
  try {
    games = await invoke("list_games");
  } catch (e) {
    toast(`Failed to load library: ${e}`, true);
    games = [];
  }
  render();
}

function render() {
  const q = $("search").value.trim().toLowerCase();
  const visible = q ? games.filter((g) => g.title.toLowerCase().includes(q)) : games;

  $("empty").classList.toggle("hidden", games.length > 0);
  const grid = $("grid");
  grid.innerHTML = "";

  for (const g of visible) {
    const card = document.createElement("article");
    card.className = "card";
    card.tabIndex = 0;

    const art = document.createElement("div");
    art.className = "art";
    if (g.cover) {
      const img = document.createElement("img");
      img.src = g.cover;
      img.alt = "";
      art.appendChild(img);
    } else {
      art.textContent = g.title.charAt(0).toUpperCase();
    }

    const label = document.createElement("div");
    label.className = "label";
    const name = document.createElement("div");
    name.className = "name";
    name.textContent = g.title;
    const sub = document.createElement("div");
    sub.className = "sub";
    sub.textContent = fmtPlaytime(g.play_seconds);
    label.append(name, sub);

    card.append(art, label);
    card.addEventListener("click", () => openDrawer(g.id));
    card.addEventListener("keydown", (e) => {
      if (e.key === "Enter") openDrawer(g.id);
    });
    grid.appendChild(card);
  }
}

function openDrawer(id) {
  const g = games.find((x) => x.id === id);
  if (!g) return;
  selectedId = id;

  $("d-title").textContent = g.title;
  $("d-exe").textContent = g.executable;
  $("d-appid").textContent = g.app_id ?? "—";
  $("d-proton").textContent = g.proton;
  $("d-play").textContent = fmtPlaytime(g.play_seconds);
  $("d-mode").value = g.steam_mode;

  $("d-launch").value = g.launch_options || "";
  populateProtons(g.proton);

  const badge = $("d-emu-badge");
  badge.textContent = g.emu_deployed ? "emulator deployed" : "not deployed";
  badge.classList.toggle("on", g.emu_deployed);

  $("drawer").classList.remove("hidden");
  void emuGameConfig.setGame(id);
}

// Proton builds are discovered once and reused for every drawer open.
let protonBuilds = null;

async function populateProtons(current) {
  const select = $("d-proton-select");
  if (protonBuilds === null) {
    try {
      protonBuilds = await invoke("list_protons");
    } catch {
      protonBuilds = [];
    }
  }
  select.innerHTML = "";
  // Always offer the auto option, which resolves a local build or lets umu fetch one.
  const names = ["GE-Proton", ...protonBuilds.map((b) => b.name)];
  for (const name of [...new Set(names)]) {
    const opt = document.createElement("option");
    opt.value = name;
    opt.textContent = name === "GE-Proton" ? "GE-Proton (auto)" : name;
    select.appendChild(opt);
  }
  select.value = current;
  if (select.value !== current) {
    // A build that is configured but no longer installed must stay visible.
    const opt = document.createElement("option");
    opt.value = current;
    opt.textContent = `${current} (not found)`;
    select.appendChild(opt);
    select.value = current;
  }
}

function closeDrawer() {
  $("drawer").classList.add("hidden");
  selectedId = null;
}

async function addGame() {
  const file = await open({
    title: "Select a game executable",
    multiple: false,
    filters: [{ name: "Game executable", extensions: ["exe", "sh", "x86_64"] }],
  });
  if (!file) return;

  const path = typeof file === "string" ? file : file.path;
  const guess = path.split("/").pop().replace(/\.(exe|sh|x86_64)$/i, "");
  const title = prompt("Game title:", guess);
  if (!title) return;

  const appIdRaw = prompt(
    "Steam AppID (optional — enables achievements, DLC and protonfixes):",
    ""
  );
  const appId = appIdRaw && /^\d+$/.test(appIdRaw.trim())
    ? parseInt(appIdRaw.trim(), 10)
    : null;

  try {
    await invoke("add_game", { title, executable: path, appId });
    toast(`Added ${title}`);
    await refresh();
  } catch (e) {
    toast(`Could not add game: ${e}`, true);
  }
}

// --- wire up ------------------------------------------------------------
$("add-btn").addEventListener("click", addGame);
$("empty-add").addEventListener("click", addGame);
$("search").addEventListener("input", render);
$("drawer-close").addEventListener("click", closeDrawer);
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closeDrawer();
});

$("d-play").addEventListener("click", async () => {
  if (!selectedId) return;
  const g = games.find((x) => x.id === selectedId);
  toast(`Launching ${g.title} through Proton…`);
  try {
    await invoke("play_game", { id: selectedId });
    toast(`${g.title} exited`);
  } catch (e) {
    toast(`Launch failed: ${e}`, true);
  }
  await refresh();
});

$("d-deploy").addEventListener("click", async () => {
  try {
    await invoke("deploy_emu", { id: selectedId });
    toast("gbe_fork deployed (originals backed up)");
    await refresh();
    openDrawer(selectedId);
  } catch (e) {
    toast(`Deploy failed: ${e}`, true);
  }
});

$("d-revert").addEventListener("click", async () => {
  try {
    await invoke("revert_emu", { id: selectedId });
    toast("Original Steamworks files restored");
    await refresh();
    openDrawer(selectedId);
  } catch (e) {
    toast(`Restore failed: ${e}`, true);
  }
});

$("d-mode").addEventListener("change", async (e) => {
  try {
    await invoke("set_steam_mode", { id: selectedId, mode: e.target.value });
    await refresh();
  } catch (err) {
    toast(`Could not change mode: ${err}`, true);
  }
});

// Persist on blur rather than each keystroke.
$("d-launch").addEventListener("change", async (e) => {
  if (!selectedId) return;
  try {
    await invoke("set_launch_options", { id: selectedId, options: e.target.value });
    toast("Launch options saved");
    await refresh();
  } catch (err) {
    toast(`Could not save launch options: ${err}`, true);
  }
});

$("d-proton-select").addEventListener("change", async (e) => {
  if (!selectedId) return;
  try {
    await invoke("set_proton", { id: selectedId, proton: e.target.value });
    toast(`Proton set to ${e.target.value}`);
    await refresh();
  } catch (err) {
    toast(`Could not set Proton: ${err}`, true);
  }
});

$("d-remove").addEventListener("click", async () => {
  const g = games.find((x) => x.id === selectedId);
  if (!confirm(`Remove ${g.title}? Original game files will be restored.`)) return;
  try {
    await invoke("remove_game", { id: selectedId });
    closeDrawer();
    await refresh();
  } catch (e) {
    toast(`Remove failed: ${e}`, true);
  }
});

// --- gbe_fork runtime panel ---------------------------------------------

function renderRuntime(st) {
  const m = st.installed;
  const warn = $("rt-warn");

  if (!m) {
    $("rt-tag").textContent = "not installed";
    $("rt-pub").textContent = "—";
    $("rt-age").textContent = "—";
    $("rt-plat").textContent = "—";
    warn.textContent =
      "No Steamworks runtime installed yet. Games in API-only or cold-client mode " +
      "cannot launch until you install it.";
    warn.classList.remove("hidden");
    return;
  }

  $("rt-tag").textContent = m.tag;
  $("rt-pub").textContent = m.published || "—";

  const age = m.installed_at
    ? Math.floor((Date.now() / 1000 - m.installed_at) / 86400)
    : null;
  $("rt-age").textContent =
    age === null ? "—" : age === 0 ? "installed today" : `${age} day${age === 1 ? "" : "s"} ago`;

  const plats = [];
  if (m.assets.some((a) => a.endsWith(".dll"))) plats.push("Windows");
  if (m.assets.some((a) => a.endsWith(".so"))) plats.push("Linux");
  $("rt-plat").textContent = plats.length ? plats.join(" + ") : "none";

  const msgs = [];
  if (st.update_available && st.latest_tag) {
    msgs.push(`A newer release is available upstream: ${st.latest_tag}.`);
  }
  if (age !== null && age > 60 && !st.latest_tag) {
    msgs.push(`This runtime is ${age} days old — check for updates.`);
  }
  if (!plats.includes("Linux")) {
    msgs.push("Native Linux games have no libsteam_api.so to swap in.");
  }

  warn.textContent = msgs.join(" ");
  warn.classList.toggle("hidden", msgs.length === 0);
}

async function openRuntime() {
  $("rt-panel").classList.remove("hidden");
  await loadSources();
  await emuGlobalConfig.refresh();
  try {
    renderRuntime(await invoke("runtime_status"));
  } catch (e) {
    toast(`Could not read runtime status: ${e}`, true);
  }
}

async function loadSources() {
  try {
    const s = await invoke("get_sources");
    $("src-repo").value = s.emulator.repo ?? "";
    $("src-api").value = s.emulator.api_base ?? "";
    $("src-win").value = s.emulator.windows_asset ?? "";
    $("src-lin").value = s.emulator.linux_asset ?? "";
    $("src-umu").value = s.umu.run_path ?? "";
  } catch (e) {
    toast(`Could not read sources: ${e}`, true);
  }
}

async function saveSources(defaults = false) {
  const emulator = defaults
    ? {
        repo: "Detanup01/gbe_fork",
        windows_asset: "emu-win-release",
        linux_asset: "emu-linux-release",
        api_base: "https://api.github.com",
      }
    : {
        repo: $("src-repo").value.trim(),
        windows_asset: $("src-win").value.trim(),
        linux_asset: $("src-lin").value.trim(),
        api_base: $("src-api").value.trim(),
      };
  const path = defaults ? "" : $("src-umu").value.trim();
  const umu = {
    run_path: path === "" ? null : path,
    project_url: "https://github.com/Open-Wine-Components/umu-launcher",
  };
  try {
    await invoke("set_sources", { emulator, umu });
    toast(defaults ? "Sources reset to defaults" : "Sources saved");
    await loadSources();
  } catch (e) {
    toast(`Could not save sources: ${e}`, true);
  }
}

$("src-save").addEventListener("click", () => saveSources(false));
$("src-reset").addEventListener("click", () => saveSources(true));

$("runtime-btn").addEventListener("click", openRuntime);
$("rt-close").addEventListener("click", () => $("rt-panel").classList.add("hidden"));

$("rt-check").addEventListener("click", async () => {
  toast("Checking upstream releases…");
  try {
    const st = await invoke("runtime_check_update");
    renderRuntime(st);
    toast(st.update_available ? `Update available: ${st.latest_tag}` : "Runtime is up to date");
  } catch (e) {
    toast(`Update check failed: ${e}`, true);
  }
});

$("rt-install").addEventListener("click", async () => {
  const platform = $("rt-platform").value;
  const btn = $("rt-install");
  btn.disabled = true;
  btn.textContent = "Downloading…";
  toast("Downloading gbe_fork from upstream — this may take a minute.");
  try {
    await invoke("runtime_install", { tag: null, platform });
    toast("Runtime installed. Games will re-deploy on next launch.");
    renderRuntime(await invoke("runtime_status"));
    await refresh();
  } catch (e) {
    toast(`Install failed: ${e}`, true);
  } finally {
    btn.disabled = false;
    btn.textContent = "Download & install";
  }
});

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") $("rt-panel").classList.add("hidden");
});

refresh();

// --- emulator settings widget ---------------------------------------------
//
// One reusable component for both layers. `game` is a game id, or null for the
// global layer — that single variable is the only difference between the two
// instances, and it can be changed at runtime with setGame().

/** Shared across instances: the common-key list is identical everywhere. */
let emuKeyCatalog = null;

async function emuKeys() {
  if (emuKeyCatalog) return emuKeyCatalog;
  try {
    emuKeyCatalog = await invoke("emu_config_keys");
  } catch {
    emuKeyCatalog = [];
  }
  return emuKeyCatalog;
}

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}

function createEmuConfigWidget({ mount, game = null, open = false }) {
  const root = typeof mount === "string" ? $(mount) : mount;
  let gameId = game;

  // --- structure, built once -------------------------------------------
  const details = el("details", "cfg");
  details.open = open;
  const summary = el("summary");
  const summaryLabel = el("span", "cfg-title");
  const summaryCount = el("span", "cfg-count");
  summary.append(summaryLabel, summaryCount);

  const hint = el("p", "hint");
  const groups = el("div", "cfg-groups");

  const keyInput = el("input", "cfg-key");
  keyInput.placeholder = "section::key";
  keyInput.spellcheck = false;
  const datalist = el("datalist");
  datalist.id = `cfg-keys-${Math.random().toString(36).slice(2, 8)}`;
  keyInput.setAttribute("list", datalist.id);

  const valueInput = el("input", "cfg-value");
  valueInput.placeholder = "value";
  valueInput.spellcheck = false;

  const addButton = el("button", "ghost cfg-apply", "Add");
  const addRow = el("div", "cfg-add");
  addRow.append(keyInput, valueInput, addButton, datalist);

  details.append(summary, hint, groups, addRow);
  root.replaceChildren(details);

  // --- behaviour --------------------------------------------------------
  const isGlobal = () => gameId == null;

  async function save(key, value) {
    await invoke("emu_config_set", { gameId, key, value });
    await refresh();
  }

  async function remove(key) {
    await invoke("emu_config_unset", { gameId, key });
    await refresh();
  }

  /** One row: key, editable value, provenance, and the right action. */
  function buildRow(key, value, ownValue, globalValue) {
    const overridden = ownValue !== undefined;
    const row = el("tr", overridden ? "own" : "inherited");

    row.append(el("td", "k", key.split("::").slice(2).join("::") || key));

    const valueCell = el("td", "v");
    const input = el("input", "cfg-inline");
    input.value = value;
    input.spellcheck = false;
    // Editing an inherited row creates an override at this layer.
    input.addEventListener("change", async () => {
      const next = input.value.trim();
      if (next === value) return;
      try {
        if (next === "") {
          await remove(key);
        } else {
          await save(key, next);
        }
      } catch (e) {
        toast(`Could not save ${key}: ${e}`, true);
        input.value = value;
      }
    });
    valueCell.appendChild(input);
    row.appendChild(valueCell);

    const action = el("td", "a");
    if (overridden) {
      const button = el("button", "cfg-del", isGlobal() ? "✕" : "↺");
      button.title = isGlobal()
        ? "Remove this setting"
        : `Revert to global${globalValue === undefined ? "" : ` (${globalValue})`}`;
      button.addEventListener("click", async () => {
        try {
          await remove(key);
        } catch (e) {
          toast(`Could not remove ${key}: ${e}`, true);
        }
      });
      action.appendChild(button);
    } else {
      const badge = el("span", "cfg-src", "global");
      badge.title = "Inherited from the global layer — edit to override for this game";
      action.appendChild(badge);
    }
    row.appendChild(action);
    return row;
  }

  async function refresh() {
    const keys = await emuKeys();
    datalist.replaceChildren(
      ...keys.map(({ key, description }) => {
        const option = el("option");
        option.value = key;
        option.label = description;
        return option;
      })
    );

    summaryLabel.textContent = isGlobal()
      ? "Emulator settings (global)"
      : "Emulator settings for this game";
    hint.innerHTML = isGlobal()
      ? "Applied to every game. A game's own entries take precedence."
      : "Overrides the global settings. Written to the game's <code>steam_settings/</code> on the next deploy.";

    let data;
    try {
      data = await invoke("emu_config_show", { gameId });
    } catch (e) {
      groups.replaceChildren(el("p", "hint", `Could not read settings: ${e}`));
      return;
    }

    const own = (isGlobal() ? data.global : data.game) || {};
    const entries = Object.entries(data.effective);
    const ownCount = Object.keys(own).length;
    summaryCount.textContent = isGlobal()
      ? `${entries.length}`
      : `${ownCount} override${ownCount === 1 ? "" : "s"}`;

    if (entries.length === 0) {
      groups.replaceChildren(el("p", "hint", "Nothing set yet."));
      return;
    }

    // Group by "<file>::<section>" so related settings sit together.
    const bySection = new Map();
    for (const [key, value] of entries) {
      const section = key.split("::").slice(0, 2).join("::");
      if (!bySection.has(section)) bySection.set(section, []);
      bySection.get(section).push([key, value]);
    }

    const rendered = [];
    for (const [section, rows] of [...bySection].sort()) {
      rendered.push(el("div", "cfg-section", section));
      const table = el("table", "cfg-table");
      const body = el("tbody");
      for (const [key, value] of rows.sort()) {
        body.appendChild(buildRow(key, value, own[key], data.global[key]));
      }
      table.appendChild(body);
      rendered.push(table);
    }
    groups.replaceChildren(...rendered);
  }

  const add = async () => {
    const key = keyInput.value.trim();
    const value = valueInput.value.trim();
    if (!key || !value) {
      toast("A key and a value are both required", true);
      return;
    }
    try {
      await save(key, value);
      keyInput.value = "";
      valueInput.value = "";
      toast("Saved — applies on the next deploy");
    } catch (e) {
      toast(`${e}`, true);
    }
  };
  addButton.addEventListener("click", add);
  for (const input of [keyInput, valueInput]) {
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") add();
    });
  }

  return {
    /** Point the widget at a different game, or null for the global layer. */
    async setGame(id) {
      gameId = id ?? null;
      await refresh();
    },
    refresh,
  };
}

const emuGlobalConfig = createEmuConfigWidget({ mount: "cfg-global", game: null });
const emuGameConfig = createEmuConfigWidget({ mount: "cfg-game", game: null });
