(() => {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;
  const $ = (id) => document.getElementById(id);

  let snap = null;
  let sv = null; // settings view from backend
  let view = "main";
  let lang = "en";
  const t = () => window.I18N[lang] || window.I18N.en;

  // ---------- helpers ----------
  function fmtMinutes(m) {
    const L = t();
    if (m == null) return "–";
    if (m < 60) return `${m} ${L.min}`;
    const h = Math.floor(m / 60);
    const r = m % 60;
    return r ? `${h} ${L.hr} ${r} ${L.min}` : `${h} ${L.hr}`;
  }

  function fmtAgo(ms) {
    if (!ms) return "";
    const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
    if (s < 10) return t().justNow;
    const rtf = new Intl.RelativeTimeFormat(lang, { style: "short", numeric: "always" });
    if (s < 60) return rtf.format(-s, "second");
    if (s < 3600) return rtf.format(-Math.round(s / 60), "minute");
    return rtf.format(-Math.round(s / 3600), "hour");
  }

  function shortSn(sn) {
    return sn.length > 6 ? "…" + sn.slice(-4) : sn;
  }

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

  // ---------- main view ----------
  function renderMain() {
    const L = t();
    const s = snap;
    if (!s) return;
    const has = s.soc != null;

    $("model").textContent = s.model || (has ? "EcoFlow" : L.noStation);
    $("ago").textContent = has ? (s.stale ? L.stale : fmtAgo(s.updatedAt)) : "";

    $("value").classList.toggle("hidden", !has);
    $("value").classList.toggle("stale", !!s.stale);
    $("empty").classList.toggle("hidden", has);

    if (has) {
      $("soc").textContent = `${s.soc}%`;
      $("bolt").classList.toggle("hidden", s.charging !== true);
      $("energy").textContent = s.capacityWh ? `~${Math.round((s.capacityWh * s.soc) / 100)} ${L.wh}` : "";
      const fill = $("fill");
      fill.style.width = `${s.soc}%`;
      const low = sv && s.soc <= sv.settings.lowThreshold && s.charging !== true;
      fill.classList.toggle("low", !!low);
      fill.classList.toggle("dim", !!s.stale);
    } else {
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
        if (cloudOn && !hint) hint = cloudText(s);
        else if (seen && seen.soc == null) hint = L.useCloud;
      } else {
        title = L.viaCloud;
        hint = cloudText(s);
      }
      $("emptyTitle").textContent = title;
      $("emptyHint").textContent = hint;
    }

    const showTiles = has && (s.wattsIn != null || s.wattsOut != null);
    $("tiles").classList.toggle("hidden", !showTiles);
    if (showTiles) {
      $("lIn").textContent = L.in;
      $("lOut").textContent = L.out;
      $("lTime").textContent = s.charging ? L.toFull : L.toEmpty;
      $("vIn").textContent = s.wattsIn != null ? `${s.wattsIn} ${L.watts}` : "–";
      $("vOut").textContent = s.wattsOut != null ? `${s.wattsOut} ${L.watts}` : "–";
      $("vTime").textContent = fmtMinutes(s.minutes);
    }

    let src = "";
    if (s.source === "ble") src = `${L.viaBle}${s.rssi != null ? ` · ${String(s.rssi).replace("-", "−")} dBm` : ""}`;
    else if (s.source === "cloud") src = L.viaCloud;
    $("source").textContent = src;
    $("openSettings").setAttribute("aria-label", L.settings);
  }

  function cloudText(s) {
    const L = t();
    let txt = L.cloudStatus[s.cloudStatus] || "";
    if (s.cloudStatus === "error" && s.cloudMessage) txt += `: ${s.cloudMessage}`;
    return txt;
  }

  // ---------- settings view ----------
  function textAll() {
    const L = t();
    const set = (id, v) => ($(id).textContent = v);
    set("tSettings", L.settings);
    set("tStation", L.station);
    set("tSnHint", L.snHint);
    set("tSources", L.sources);
    set("tBle", L.ble);
    set("tBleHint", L.bleHint);
    set("tCloud", L.cloud);
    set("tCloudHint", L.cloudHint);
    set("signIn", L.signIn);
    set("tNotifications", L.notifications);
    set("tNotifyLow", L.notifyLow);
    set("tThreshold", L.threshold);
    set("tNotifyFull", L.notifyFull);
    set("tGeneral", L.general);
    set("tLanguage", L.language);
    set("tAutostart", L.autostart);
    set("quit", L.quit);
    $("lang").options[0].textContent = L.langAuto;
    $("email").placeholder = L.email;
    $("snInput").placeholder = L.snPlaceholder;
    $("back").setAttribute("aria-label", L.back);
    const reg = $("region");
    reg.innerHTML = "";
    [["eu", L.regionEu], ["global", L.regionGlobal], ["america", L.regionAmerica]].forEach(([v, n]) => {
      const o = document.createElement("option");
      o.value = v;
      o.textContent = n;
      reg.appendChild(o);
    });
    if (sv) reg.value = sv.settings.cloudRegion;
  }

  let snMode = false; // manual serial entry shown
  let cloudDirty = false; // email / region edited but not submitted

  function renderDevices() {
    if (!sv || !snap) return;
    const L = t();
    const sel = $("deviceSelect");
    const cur = sv.settings.deviceSn;
    const items = [["", L.autoNearest]];
    const seen = new Set();
    (snap.devices || []).forEach((d) => {
      seen.add(d.sn);
      const pct = d.soc != null ? ` · ${d.soc}%` : "";
      items.push([d.sn, `${d.model} ${shortSn(d.sn)}${pct}`]);
    });
    if (cur && !seen.has(cur)) items.push([cur, `${snap.sn === cur && snap.model ? snap.model : "EcoFlow"} ${shortSn(cur)}`]);
    items.push(["__other", L.otherSn]);

    const sig = JSON.stringify(items) + "|" + cur + "|" + snMode;
    if (sel.dataset.sig === sig) return;
    // Don't rebuild the list under the user's cursor; the next snapshot will do it.
    if (document.activeElement === sel && sel.dataset.sig) return;
    sel.dataset.sig = sig;
    sel.innerHTML = "";
    items.forEach(([v, n]) => {
      const o = document.createElement("option");
      o.value = v;
      o.textContent = n;
      sel.appendChild(o);
    });
    sel.value = snMode ? "__other" : cur;
    $("snInput").classList.toggle("hidden", !snMode);
    $("tSnHint").classList.toggle("hidden", !snMode);
  }

  function renderSettings() {
    if (!sv) return;
    const s = sv.settings;
    const L = t();
    $("bleEnabled").checked = s.bleEnabled;
    $("cloudEnabled").checked = s.cloudEnabled;
    $("cloudBox").classList.toggle("hidden", !s.cloudEnabled);
    // Keep what the user typed but has not submitted yet (other switches save instantly).
    if (!cloudDirty) {
      $("email").value = s.cloudEmail;
      $("region").value = s.cloudRegion;
    }
    $("password").placeholder = sv.hasPassword ? `${L.password} – ${L.passwordSaved}` : L.password;
    $("notifyLow").checked = s.notifyLow;
    $("lowThreshold").value = s.lowThreshold;
    $("thresholdVal").textContent = `${s.lowThreshold}%`;
    $("thresholdRow").classList.toggle("hidden", !s.notifyLow);
    $("notifyFull").checked = s.notifyFull;
    $("lang").value = s.lang;
    $("autostart").checked = s.autostart;
    $("version").textContent = `${L.version} ${sv.version}`;
    renderCloudStatus();
    renderDevices();
  }

  function renderCloudStatus() {
    if (!snap) return;
    const el = $("cloudStatus");
    el.textContent = cloudText(snap);
    el.classList.toggle("ok", snap.cloudStatus === "online");
    el.classList.toggle("err", ["auth_error", "error"].includes(snap.cloudStatus));
  }

  async function save(patch, password) {
    if (!sv) return;
    const settings = Object.assign({}, sv.settings, patch);
    try {
      // A language change arrives with the next snapshot (resolved on the backend).
      sv = await invoke("save_settings", { settings, password: password || null });
      renderSettings();
      resize();
      return true;
    } catch (e) {
      const el = $("cloudStatus");
      el.textContent = e === "email" ? t().errEmail : String(e);
      el.classList.remove("ok");
      el.classList.add("err");
      return false;
    }
  }

  function setView(v) {
    view = v;
    $("main").classList.toggle("hidden", v !== "main");
    $("settingsView").classList.toggle("hidden", v !== "settings");
    if (v === "settings") {
      snMode = false;
      cloudDirty = false;
      $("password").value = "";
      $("deviceSelect").dataset.sig = "";
      renderSettings();
      $("scroll").scrollTop = 0;
    }
    resize();
  }

  // ---------- overlay scrollbar (appears while scrolling, then fades) ----------
  let thumbTimer = null;
  function updateThumb(show) {
    const sc = $("scroll");
    const th = $("thumb");
    const ratio = sc.clientHeight / sc.scrollHeight;
    if (ratio >= 1) {
      th.classList.remove("show");
      return;
    }
    const h = Math.max(24, sc.clientHeight * ratio);
    const top = (sc.scrollTop / (sc.scrollHeight - sc.clientHeight)) * (sc.clientHeight - h);
    th.style.height = `${h}px`;
    th.style.transform = `translateY(${top}px)`;
    if (show) {
      th.classList.add("show");
      clearTimeout(thumbTimer);
      thumbTimer = setTimeout(() => th.classList.remove("show"), 800);
    }
  }

  // ---------- wiring ----------
  function applySnapshot(s) {
    const langChanged = s.lang !== lang;
    snap = s;
    lang = s.lang;
    document.documentElement.lang = lang;
    if (langChanged) textAll();
    renderMain();
    if (view === "settings") {
      if (langChanged) renderSettings();
      renderCloudStatus();
      renderDevices();
    }
    resize();
  }

  $("openSettings").onclick = () => setView("settings");
  $("back").onclick = () => setView("main");
  $("quit").onclick = () => invoke("quit_app");
  $("scroll").addEventListener("scroll", () => updateThumb(true), { passive: true });

  $("deviceSelect").onchange = (e) => {
    const v = e.target.value;
    if (v === "__other") {
      snMode = true;
      $("snInput").value = sv.settings.deviceSn;
      $("deviceSelect").dataset.sig = "";
      renderDevices();
      $("snInput").focus();
      resize();
      return;
    }
    snMode = false;
    e.target.blur(); // lets the list refresh again (it is not rebuilt while focused)
    save({ deviceSn: v });
  };
  const commitSn = () => {
    const v = $("snInput").value.trim().toUpperCase();
    if (v !== sv.settings.deviceSn) save({ deviceSn: v });
  };
  $("snInput").onchange = commitSn;
  $("snInput").onkeydown = (e) => {
    if (e.key === "Enter") commitSn();
  };

  $("bleEnabled").onchange = (e) => save({ bleEnabled: e.target.checked });
  $("cloudEnabled").onchange = (e) => save({ cloudEnabled: e.target.checked });
  $("email").oninput = () => (cloudDirty = true);
  $("region").onchange = () => (cloudDirty = true);
  $("signIn").onclick = async () => {
    const pass = $("password").value;
    const ok = await save({ cloudEmail: $("email").value.trim(), cloudRegion: $("region").value }, pass);
    if (ok) {
      cloudDirty = false;
      $("password").value = "";
      renderSettings();
    }
  };
  $("password").onkeydown = (e) => {
    if (e.key === "Enter") $("signIn").click();
  };
  $("notifyLow").onchange = (e) => save({ notifyLow: e.target.checked });
  $("lowThreshold").oninput = (e) => ($("thresholdVal").textContent = `${e.target.value}%`);
  $("lowThreshold").onchange = (e) => save({ lowThreshold: Number(e.target.value) });
  $("notifyFull").onchange = (e) => save({ notifyFull: e.target.checked });
  $("lang").onchange = (e) => save({ lang: e.target.value });
  $("autostart").onchange = (e) => save({ autostart: e.target.checked });

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      if (view === "settings") setView("main");
      else invoke("hide_popup");
    }
  });
  document.addEventListener("contextmenu", (e) => e.preventDefault());

  listen("snapshot", (e) => applySnapshot(e.payload));
  listen("popup-shown", async (e) => {
    const [s, settings] = await Promise.all([invoke("get_snapshot"), invoke("get_settings")]);
    sv = settings;
    applySnapshot(s);
    setView(e.payload ? "settings" : "main");
    resize(true);
  });

  // While the popup is open, refresh from the backend so "updated … ago" is truthful
  // (pushed snapshots are throttled; adverts arrive about once a second).
  setInterval(() => {
    if (document.hidden) return;
    invoke("get_snapshot").then(applySnapshot).catch(() => {});
  }, 5000);

  (async () => {
    sv = await invoke("get_settings");
    const s = await invoke("get_snapshot");
    lang = "";
    applySnapshot(s);
  })();
})();
