// Shared by the popup and the settings window.
window.SB = (() => {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;
  const $ = (id) => document.getElementById(id);
  let lang = "en";

  const t = () => window.I18N[lang] || window.I18N.en;

  function setLang(l) {
    if (!l || l === lang) return false;
    lang = l;
    document.documentElement.lang = l;
    return true;
  }

  async function initPlatform() {
    let p = { os: "linux", backdrop: false, accentOnLight: "", accentOnDark: "" };
    try {
      p = await invoke("get_platform");
    } catch (e) {}
    const root = document.documentElement;
    root.classList.add(p.os === "macos" ? "mac" : "win");
    if (p.backdrop) root.classList.add("backdrop");
    if (p.accentOnLight) root.style.setProperty("--accent-on-light", p.accentOnLight);
    if (p.accentOnDark) root.style.setProperty("--accent-on-dark", p.accentOnDark);
    applyIcons();
    return p;
  }

  const isMac = () => document.documentElement.classList.contains("mac");

  /** Fills every [data-ico] with the icon of the current platform's icon set. */
  function applyIcons() {
    const set = (window.SB_ICONS || {})[isMac() ? "mac" : "win"] || {};
    document.querySelectorAll("[data-ico]").forEach((el) => {
      const svg = set[el.dataset.ico];
      if (svg && el.dataset.icoDone !== "1") {
        el.innerHTML = svg;
        el.dataset.icoDone = "1";
      }
    });
  }

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

  function cloudText(s) {
    const L = t();
    let txt = L.cloudStatus[s.cloudStatus] || "";
    if (s.cloudStatus === "error" && s.cloudMessage) txt += `: ${s.cloudMessage}`;
    return txt;
  }

  /** Overlay scrollbar for a scrolling element: shows while scrolling, then fades. */
  function overlayScrollbar(el) {
    const th = document.createElement("div");
    th.className = "thumb";
    document.body.appendChild(th);
    let timer = null;
    const update = (show) => {
      const ratio = el.clientHeight / el.scrollHeight;
      if (ratio >= 1) {
        th.classList.remove("show");
        return;
      }
      const r = el.getBoundingClientRect();
      const h = Math.max(24, el.clientHeight * ratio);
      const top = r.top + (el.scrollTop / (el.scrollHeight - el.clientHeight)) * (el.clientHeight - h);
      th.style.height = `${h}px`;
      th.style.transform = `translateY(${top}px)`;
      if (show) {
        th.classList.add("show");
        clearTimeout(timer);
        timer = setTimeout(() => th.classList.remove("show"), 800);
      }
    };
    el.addEventListener("scroll", () => update(true), { passive: true });
    return update;
  }

  return { invoke, listen, $, t, setLang, getLang: () => lang, initPlatform, isMac, fmtMinutes, fmtAgo, cloudText, overlayScrollbar };
})();
