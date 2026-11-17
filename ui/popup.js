(async () => {
  const { invoke, listen, $, t } = SB;
  const RING = 2 * Math.PI * 24;

  let snap = null;
  let lowThreshold = 20;

  // One measurement per frame, and only tell the backend when the height changed.
  let resizePending = false;
  let lastHeight = 0;
  function resize(force) {
    if (force) lastHeight = 0;
    if (resizePending) return;
    resizePending = true;
    const run = () => {
      if (!resizePending) return;
      resizePending = false;
      const h = Math.ceil($("card").getBoundingClientRect().height);
      if (h === lastHeight) return;
      lastHeight = h;
      invoke("set_popup_height", { height: h }).catch(() => {});
    };
    // rAF is paused while the window is hidden; the timer makes sure it still runs.
    requestAnimationFrame(run);
    setTimeout(run, 120);
  }

  /** Everything both layouts need, computed once. */
  function model(s) {
    const L = t();
    const mac = SB.isMac();
    const has = s.soc != null;
    const m = { has, name: s.model || (has ? "EcoFlow" : L.noStation) };
    m.low = has && s.soc <= lowThreshold && s.charging !== true;
    m.energy = has && s.capacityWh ? `~${Math.round((s.capacityWh * s.soc) / 100)} ${L.wh}` : "";

    const parts = [];
    if (s.charging === true) {
      parts.push(L.charging.charAt(0).toUpperCase() + L.charging.slice(1));
      if (s.minutes != null) parts.push(`${L.toFullIn} ${SB.fmtMinutes(s.minutes)}`);
    } else if (s.minutes != null) {
      parts.push(`${L.remainingIn} ${SB.fmtMinutes(s.minutes)}`);
    }
    m.status = s.stale ? L.stale : parts.join(mac ? " – " : " · ");
    m.ago = has && !s.stale ? SB.fmtAgo(s.updatedAt) : "";

    m.source = s.source === "ble"
      ? `${L.viaBle}${!mac && s.rssi != null ? ` · ${String(s.rssi).replace("-", "−")} dBm` : ""}`
      : s.source === "cloud" ? L.viaCloud : "";
    m.power = has && (s.wattsIn != null || s.wattsOut != null || s.voltsIn != null);
    m.in = s.wattsIn != null ? `${s.wattsIn} ${L.watts}` : "–";
    m.out = s.wattsOut != null ? `${s.wattsOut} ${L.watts}` : "–";
    // Grid voltage is worth showing even at 0 (no grid); output 0 just means AC is off.
    m.inV = s.voltsIn != null ? `${s.voltsIn} ${L.volts}` : "";
    m.outV = s.voltsOut ? `${s.voltsOut} ${L.volts}` : "";
    m.soh = s.soh != null ? `${s.soh}%` : "";
    m.cycles = s.cycles != null ? String(s.cycles) : "";
    m.health = has && (m.soh !== "" || m.cycles !== "");

    // Empty state
    let title = L.searching;
    let hint = "";
    const bleOn = s.bleStatus !== "off";
    const cloudOn = s.cloudStatus !== "off";
    if (!bleOn && !cloudOn) {
      title = L.noStation;
      hint = L.noSourceHint;
    } else if (bleOn) {
      const seen = (s.devices || []).find((d) => d.sn === s.sn);
      if (s.bleStatus === "no_adapter") title = L.noAdapter;
      else if (s.bleStatus === "bt_off") title = L.btOff;
      else if (s.bleStatus === "no_permission") {
        title = L.noPermission;
        hint = L.noPermissionHint;
      } else if (s.bleStatus === "error") title = L.bleError;
      else if (seen && seen.soc == null) title = L.bleNoSoc;
      if (cloudOn && !hint) hint = SB.cloudText(s);
      else if (!cloudOn && seen && seen.soc == null) hint = L.useCloud;
    } else {
      title = L.viaCloud;
      hint = SB.cloudText(s);
    }
    m.emptyTitle = title;
    m.emptyHint = hint;
    return m;
  }

  function render() {
    if (!snap) return;
    const s = snap;
    const L = t();
    const m = model(s);

    // Windows
    $("wModel").textContent = m.name;
    $("wAgo").textContent = m.ago;
    $("wValue").classList.toggle("hidden", !m.has);
    $("wEmpty").classList.toggle("hidden", m.has);
    if (m.has) {
      $("wSoc").textContent = `${s.soc}%`;
      const arc = $("wArc");
      arc.style.strokeDasharray = `${RING}`;
      arc.style.strokeDashoffset = `${RING * (1 - s.soc / 100)}`;
      arc.classList.toggle("low", m.low);
      arc.classList.toggle("dim", !!s.stale);
      $("wBolt").classList.toggle("hidden", s.charging !== true);
      // One line: the charging / time status when known, otherwise the stored energy.
      $("wStatus").textContent = m.status || m.energy;
    }
    $("wEmptyTitle").textContent = m.emptyTitle;
    $("wEmptyHint").textContent = m.emptyHint;
    $("wTiles").classList.toggle("hidden", !m.power);
    $("wLIn").textContent = L.in;
    $("wLOut").textContent = L.out;
    $("wIn").textContent = m.in;
    $("wOut").textContent = m.out;
    $("wInV").textContent = m.inV;
    $("wOutV").textContent = m.outV;
    $("wHealth").classList.toggle("hidden", !m.health);
    $("wHealthL").textContent = L.health;
    $("wHealthV").textContent = [m.soh, m.cycles && `${L.cycles}: ${m.cycles}`].filter(Boolean).join(" · ");
    $("wSource").textContent = m.source;
    $("wSettings").title = L.settings;
    $("wSettings").setAttribute("aria-label", L.settings);

    // macOS
    $("mModel").textContent = m.name;
    $("mSource").textContent = m.source;
    $("mValue").classList.toggle("hidden", !m.has);
    $("mEmpty").classList.toggle("hidden", m.has);
    if (m.has) {
      $("mSoc").textContent = `${s.soc}%`;
      $("mBolt").classList.toggle("hidden", s.charging !== true);
      $("mEnergy").textContent = m.energy;
      const fill = $("mFill");
      fill.style.width = `${s.soc}%`;
      fill.classList.toggle("charging", s.charging === true);
      fill.classList.toggle("low", m.low);
      fill.classList.toggle("dim", !!s.stale);
      let status = m.status || (m.ago ? `${L.updated} ${m.ago}` : "");
      status = status.charAt(0).toUpperCase() + status.slice(1);
      $("mStatus").textContent = status;
      $("mStatus").classList.toggle("hidden", !status);
    }
    $("mEmptyTitle").textContent = m.emptyTitle;
    $("mEmptyHint").textContent = m.emptyHint;
    $("mPower").classList.toggle("hidden", !m.power);
    $("mLIn").textContent = L.in;
    $("mLOut").textContent = L.out;
    $("mIn").textContent = [m.in, m.inV].filter(Boolean).join(" · ");
    $("mOut").textContent = [m.out, m.outV].filter(Boolean).join(" · ");
    $("mHealth").classList.toggle("hidden", !m.health);
    $("mSohRow").classList.toggle("hidden", m.soh === "");
    $("mCyclesRow").classList.toggle("hidden", m.cycles === "");
    $("mLSoh").textContent = L.health;
    $("mLCycles").textContent = L.cycles;
    $("mSoh").textContent = m.soh;
    $("mCycles").textContent = m.cycles;
    $("mSettingsLabel").textContent = L.menuSettings;
    $("mQuitLabel").textContent = L.menuQuit;

    resize();
  }

  function apply(s) {
    snap = s;
    if (s.lowThreshold != null) lowThreshold = s.lowThreshold;
    SB.setLang(s.lang);
    render();
  }

  const openSettings = () => invoke("open_settings");
  $("wSettings").onclick = openSettings;
  $("mSettings").onclick = openSettings;
  $("mQuit").onclick = () => invoke("quit_app");

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") invoke("hide_popup");
    if (e.metaKey && e.key === ",") {
      e.preventDefault();
      openSettings();
    }
    if (e.metaKey && (e.key === "q" || e.key === "Q")) {
      e.preventDefault();
      invoke("quit_app");
    }
  });
  document.addEventListener("contextmenu", (e) => e.preventDefault());

  await SB.initPlatform();

  listen("snapshot", (e) => apply(e.payload));
  // The window is about to be shown: render fresh data, measure, and let the backend
  // show the window at exactly this size (resizing a visible window makes the system
  // shadow jump). Layout works in a hidden page; only animation frames are paused.
  listen("popup-prepare", async (e) => {
    const id = e.payload;
    let height = Math.ceil($("card").getBoundingClientRect().height);
    try {
      apply(await invoke("get_snapshot"));
      height = Math.ceil($("card").getBoundingClientRect().height);
      lastHeight = height;
    } finally {
      invoke("reveal_popup", { id, height }).catch(() => {});
    }
    SB.initPlatform(); // picks up a changed Windows accent colour
  });

  // While the popup is open, refresh so "updated … ago" stays truthful.
  setInterval(() => {
    if (document.hidden) return;
    invoke("get_snapshot").then(apply).catch(() => {});
  }, 5000);

  // The snapshot carries everything the popup needs (no settings / keychain lookups).
  apply(await invoke("get_snapshot"));
})();
