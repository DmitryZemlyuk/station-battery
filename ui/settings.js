(async () => {
  const { invoke, listen, $, t } = SB;

  let sv = null; // settings view from the backend
  let snap = null;
  let tab = "general";
  let snMode = false; // manual serial entry shown
  let cloudDirty = false; // email / region edited but not submitted
  let passwordDirty = false; // something typed into the password field
  let revealed = false;

  // ---------- texts ----------
  function textAll() {
    const L = t();
    document.querySelectorAll("[data-t]").forEach((el) => {
      const v = L[el.dataset.t];
      if (typeof v === "string") el.textContent = v;
    });
    $("email").placeholder = "name@example.com";
    $("snInput").placeholder = L.snPlaceholder;
    const reg = $("region");
    const cur = reg.value || (sv && sv.settings.cloudRegion) || "eu";
    reg.innerHTML = "";
    [["eu", L.regionEu], ["global", L.regionGlobal], ["america", L.regionAmerica]].forEach(([v, n]) => {
      const o = document.createElement("option");
      o.value = v;
      o.textContent = n;
      reg.appendChild(o);
    });
    reg.value = cur;
    // Tray / menu bar item: macOS has no separate battery-glyph style (its battery
    // already carries the percentage), so that option exists on Windows only.
    const tray = $("trayStyle");
    if (SB.isMac()) {
      const bat = tray.querySelector('option[value="battery"]');
      if (bat) bat.remove();
    }
    tray.querySelector('option[value="number"]').textContent = SB.isMac() ? L.trayCharge : L.trayNumber;
    $("deviceSelect").dataset.sig = "";
    renderSignIn();
    renderTab();
  }

  // ---------- render ----------
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
      items.push([d.sn, `${d.model} …${d.sn.slice(-4)}${pct}`]);
    });
    if (cur && !seen.has(cur)) items.push([cur, `${snap.sn === cur && snap.model ? snap.model : "EcoFlow"} …${cur.slice(-4)}`]);
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
    $("snRow").classList.toggle("hidden", !snMode);
  }

  function setToggle(id, on) {
    $(id).checked = on;
    const st = document.querySelector(`.state[data-for="${id}"]`);
    if (st) st.textContent = on ? t().on : t().off;
  }

  function renderCloudStatus() {
    if (!snap || !sv) return;
    const L = t();
    const text = SB.cloudText(snap);
    $("cloudDesc").textContent = sv.settings.cloudEnabled && text ? text : L.cloudHint;
    // The header already shows the connection state; this line is only for save errors.
    const el = $("cloudStatus");
    if (!el.dataset.error) el.textContent = "";
    el.classList.toggle("err", !!el.dataset.error);
    $("cloudDesc").classList.toggle("ok", snap.cloudStatus === "online");
    $("cloudDesc").classList.toggle("err", ["auth_error", "error"].includes(snap.cloudStatus));
  }

  function render() {
    if (!sv) return;
    const s = sv.settings;
    const L = t();
    setToggle("bleEnabled", s.bleEnabled);
    setToggle("cloudEnabled", s.cloudEnabled);
    setToggle("notifyLow", s.notifyLow);
    setToggle("notifyFull", s.notifyFull);
    setToggle("autostart", s.autostart);
    document.querySelectorAll(".cloud-sub").forEach((el) => el.classList.toggle("hidden", !s.cloudEnabled));
    if (!cloudDirty) {
      $("email").value = s.cloudEmail;
      $("region").value = s.cloudRegion;
    }
    $("password").placeholder = sv.hasPassword ? L.passwordSaved : "";
    renderSignIn();
    $("lowThreshold").value = s.lowThreshold;
    $("thresholdVal").textContent = `${s.lowThreshold}%`;
    $("thresholdRow").classList.toggle("hidden", !s.notifyLow);
    $("lang").value = s.lang;
    // "battery" is not offered on macOS: show it as the default there.
    $("trayStyle").value = SB.isMac() && s.trayStyle === "battery" ? "number" : s.trayStyle;
    $("trayHint").textContent = s.trayStyle === "voltage" ? L.trayVoltageHint : SB.isMac() ? "" : L.trayIconHint;
    $("version").textContent = `Station Battery ${sv.version}`;
    renderCloudStatus();
    renderDevices();
    fit();
  }

  // ---------- macOS tabs ----------
  function renderTab() {
    document.querySelectorAll(".mac-tab").forEach((b) => b.classList.toggle("active", b.dataset.tab === tab));
    document.querySelectorAll(".pane").forEach((p) => p.classList.toggle("active", p.dataset.pane === tab));
    const L = t();
    $("macTitle").textContent = { general: L.tabGeneral, sources: L.tabSources, notifications: L.tabNotifications }[tab];
    fit();
  }
  document.querySelectorAll(".mac-tab").forEach((b) => {
    b.onclick = () => {
      tab = b.dataset.tab;
      renderTab();
    };
  });

  // macOS: the window height follows the content of the selected tab.
  let fitPending = false;
  let lastFit = 0;
  function fitNow() {
    fitPending = false;
    if (!SB.isMac()) return;
    const h = Math.ceil(document.querySelector(".mac-top").getBoundingClientRect().height + $("scroll").scrollHeight);
    if (Math.abs(h - lastFit) < 1) return;
    lastFit = h;
    invoke("set_window_height", { height: h }).catch(() => {});
  }
  function fit() {
    if (!SB.isMac() || fitPending) return;
    fitPending = true;
    setTimeout(fitNow, 0); // a timer, not rAF: it must also work while the window is hidden
  }

  // ---------- saving ----------
  async function save(patch, password) {
    if (!sv) return false;
    const settings = Object.assign({}, sv.settings, patch);
    try {
      sv = await invoke("save_settings", { settings, password: password || null });
      delete $("cloudStatus").dataset.error;
      render();
      return true;
    } catch (e) {
      const el = $("cloudStatus");
      el.dataset.error = "1";
      el.textContent = e === "email" ? t().errEmail : String(e);
      return false;
    }
  }

  $("deviceSelect").onchange = (e) => {
    const v = e.target.value;
    if (v === "__other") {
      snMode = true;
      $("snInput").value = sv.settings.deviceSn;
      $("deviceSelect").dataset.sig = "";
      renderDevices();
      $("snInput").focus();
      fit();
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
  // Signed in (a password is saved) and nothing edited: the button signs out.
  // Any edit to the email, password or region turns it back into "Sign in".
  function signedIn() {
    return !!(sv && sv.hasPassword && !cloudDirty && !passwordDirty);
  }
  function renderSignIn() {
    const L = t();
    const b = $("signIn");
    const out = signedIn();
    b.textContent = out ? L.signOut : L.signIn;
    b.classList.toggle("accent", !out);
  }
  $("email").oninput = () => {
    cloudDirty = true;
    renderSignIn();
  };
  $("region").onchange = () => {
    cloudDirty = true;
    renderSignIn();
  };
  $("password").oninput = () => {
    passwordDirty = $("password").value !== "";
    renderSignIn();
  };
  $("signIn").onclick = async () => {
    if (signedIn()) {
      sv = await invoke("sign_out");
      delete $("cloudStatus").dataset.error;
      render();
      return;
    }
    const pass = $("password").value;
    const ok = await save({ cloudEmail: $("email").value.trim(), cloudRegion: $("region").value }, pass);
    if (ok) {
      cloudDirty = false;
      passwordDirty = false;
      $("password").value = "";
      render();
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
  $("trayStyle").onchange = (e) => save({ trayStyle: e.target.value });
  $("quit").onclick = () => invoke("quit_app");
  document.addEventListener("contextmenu", (e) => {
    if (!(e.target instanceof HTMLInputElement)) e.preventDefault();
  });

  // ---------- start ----------
  await SB.initPlatform();
  if (!SB.isMac()) SB.overlayScrollbar($("scroll"));

  const apply = (s) => {
    snap = s;
    if (SB.setLang(s.lang)) textAll();
    renderCloudStatus();
    renderDevices();
  };
  listen("snapshot", (e) => apply(e.payload));
  listen("settings-changed", (e) => {
    sv = e.payload;
    render();
  });

  sv = await invoke("get_settings");
  const first = await invoke("get_snapshot");
  SB.setLang(first.lang);
  snap = first;
  textAll();
  render();
  // The window is created hidden, and hidden webviews don't run requestAnimationFrame,
  // so reveal it from a timer once the first layout is done.
  setTimeout(() => {
    if (!revealed) {
      revealed = true;
      fitNow();
      invoke("reveal_window");
    }
  }, 30);
})();
