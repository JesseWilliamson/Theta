/* Theta site: progressive enhancement only. Every page works without this file. */
(function () {
  "use strict";

  // The one place the repository URL lives for scripts. After a rename run `python3 tools/set_repo.py NEW_URL`,
  // which updates this constant and the plain href fallbacks in the HTML.
  var REPO = "https://github.com/JesseWilliamson/Quarry";

  var doc = document;
  var root = doc.documentElement;
  function $(sel, ctx) { return (ctx || doc).querySelector(sel); }
  function $$(sel, ctx) { return Array.prototype.slice.call((ctx || doc).querySelectorAll(sel)); }
  function store(key, val) {
    try { if (val === undefined) return localStorage.getItem(key); localStorage.setItem(key, val); } catch (e) {}
    return null;
  }

  /* ---------- repo links: <a data-repo="/releases/latest"> ---------- */
  $$("a[data-repo]").forEach(function (a) { a.href = REPO + a.getAttribute("data-repo"); });

  /* ---------- theme toggle ---------- */
  var themeBtn = $(".theme-toggle");
  function effectiveTheme() {
    var t = root.getAttribute("data-theme");
    if (t) return t;
    return window.matchMedia && matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  }
  function syncThemeUi() {
    if (!themeBtn) return;
    var now = effectiveTheme();
    themeBtn.setAttribute("aria-label", "Switch to " + (now === "dark" ? "light" : "dark") + " theme");
    var meta = $('meta[name="theme-color"]');
    if (meta) meta.setAttribute("content", now === "dark" ? "#181b1f" : "#f6f4ef");
  }
  if (themeBtn) {
    themeBtn.addEventListener("click", function () {
      var next = effectiveTheme() === "dark" ? "light" : "dark";
      root.setAttribute("data-theme", next);
      store("theta-theme", next);
      syncThemeUi();
    });
    syncThemeUi();
  }

  /* ---------- mobile menu ---------- */
  var menuBtn = $(".menu-btn"), links = $("#nav-links");
  if (menuBtn && links) {
    function setMenu(open) {
      links.classList.toggle("open", open);
      menuBtn.setAttribute("aria-expanded", open ? "true" : "false");
    }
    menuBtn.addEventListener("click", function () { setMenu(!links.classList.contains("open")); });
    links.addEventListener("click", function (e) { if (e.target.closest("a")) setMenu(false); });
    doc.addEventListener("keydown", function (e) {
      if (e.key === "Escape" && links.classList.contains("open")) { setMenu(false); menuBtn.focus(); }
    });
  }

  /* ---------- tabs (WAI-ARIA tabs pattern, automatic activation) ---------- */
  var tabGroups = $$("[data-tabs]");
  tabGroups.forEach(function (group) {
    var tabs = $$('[role="tab"]', group);
    var panels = tabs.map(function (t) { return doc.getElementById(t.getAttribute("aria-controls")); });
    function select(i, focus) {
      tabs.forEach(function (t, j) {
        var on = i === j;
        t.setAttribute("aria-selected", on ? "true" : "false");
        t.tabIndex = on ? 0 : -1;
        if (panels[j]) panels[j].hidden = !on;
      });
      if (focus) tabs[i].focus();
    }
    tabs.forEach(function (t, i) {
      t.addEventListener("click", function () {
        select(i, false);
        if (group.hasAttribute("data-hash")) history.replaceState(null, "", "#" + t.id.replace("tab-", ""));
      });
      t.addEventListener("keydown", function (e) {
        var n = tabs.length, to = -1;
        if (e.key === "ArrowRight") to = (i + 1) % n;
        else if (e.key === "ArrowLeft") to = (i - 1 + n) % n;
        else if (e.key === "Home") to = 0;
        else if (e.key === "End") to = n - 1;
        if (to >= 0) { e.preventDefault(); select(to, true); tabs[to].click(); }
      });
    });
    var start = 0;
    if (group.hasAttribute("data-hash")) {
      var h = location.hash.replace("#", "");
      tabs.forEach(function (t, i) { if (t.id === "tab-" + h) start = i; });
    }
    select(start, false);
    group._select = select;
  });

  /* ---------- OS-aware download hint ---------- */
  var hint = $("#dl-hint");
  if (hint) {
    var ua = navigator.userAgent || "";
    var plat = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || "";
    var os = "other";
    if (/Android|iPhone|iPad|iPod/.test(ua)) os = "mobile";
    else if (/Mac/i.test(plat) || /Mac OS X/.test(ua)) os = "mac";
    else if (/Win/i.test(plat) || /Windows/.test(ua)) os = "windows";
    else if (/Linux|X11|CrOS/i.test(plat + ua)) os = "linux";

    var names = {
      "linux": ["theta-<version>-linux-x86_64.tar.gz", "Theta-<version>-linux-x86_64.AppImage"],
      "windows": ["theta-<version>-windows-x86_64.zip"],
      "mac-arm": ["Theta-<version>-macos-arm64.zip"],
      "mac-intel": ["Theta-<version>-macos-x86_64.zip"]
    };
    function say(parts) {
      hint.textContent = "";
      parts.forEach(function (p) {
        if (typeof p === "string") hint.appendChild(doc.createTextNode(p));
        else { var s = doc.createElement("strong"); s.textContent = p.b; hint.appendChild(s); }
      });
    }
    function mark(keys) {
      $$("tr[data-os]").forEach(function (tr) { tr.classList.toggle("detected", keys.indexOf(tr.getAttribute("data-os")) >= 0); });
    }
    if (os === "linux") {
      mark(["linux"]);
      say(["Detected ", { b: "Linux" }, ". Get ", { b: names.linux[0] }, " or the ", { b: "AppImage" }, ". Alpha: hand-tested on X11."]);
    } else if (os === "windows") {
      mark(["windows"]);
      say(["Detected ", { b: "Windows" }, ". Get ", { b: names.windows[0] }, ". Alpha: unsigned, and not yet hand-tested on Windows."]);
    } else if (os === "mac") {
      mark(["mac-arm", "mac-intel"]);
      var macBoth = function () {
        say(["Detected ", { b: "macOS" }, ". Get ", { b: names["mac-arm"][0] }, " (Apple Silicon) or ", { b: names["mac-intel"][0] }, " (Intel). Alpha: unsigned, and not yet hand-tested on macOS."]);
      };
      macBoth();
      if (navigator.userAgentData && navigator.userAgentData.getHighEntropyValues) {
        navigator.userAgentData.getHighEntropyValues(["architecture"]).then(function (v) {
          var arm = v.architecture === "arm";
          mark([arm ? "mac-arm" : "mac-intel"]);
          say(["Detected ", { b: "macOS" }, (arm ? " on Apple Silicon" : " on Intel"), ". Get ", { b: names[arm ? "mac-arm" : "mac-intel"][0] }, ". Alpha: unsigned, and not yet hand-tested on macOS."]);
        }).catch(macBoth);
      }
    } else if (os === "mobile") {
      say(["Theta is a desktop app. Builds for ", { b: "Linux" }, ", ", { b: "macOS" }, " and ", { b: "Windows" }, " are on the releases page."]);
    }
  }

  /* ---------- editor mock: type "join ", open the popup, accept with Tab ---------- */
  $$("[data-demo]").forEach(function (fig) {
    var typeEl = $("[data-type]", fig), pop = $("[data-pop]", fig), ghost = $("[data-ghost]", fig);
    var tip = $("[data-tip]", fig), replay = $("[data-replay]", fig), col = $("[data-col]", fig);
    var reduce = window.matchMedia && matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (reduce || !("IntersectionObserver" in window) || !typeEl || !pop || !ghost) return; // static state stays
    root.classList.add("js-anim");
    var token = 0;
    function wait(ms) { return new Promise(function (r) { setTimeout(r, ms); }); }
    async function play() {
      var me = ++token;
      typeEl.textContent = ""; pop.hidden = true; pop.classList.remove("closed");
      ghost.hidden = true; ghost.classList.remove("in"); if (tip) tip.classList.remove("on");
      if (col) col.textContent = "Ln 3, Col 6";
      await wait(500);
      var kw = doc.createElement("span"); kw.className = "kw"; typeEl.appendChild(kw);
      var word = "join";
      for (var i = 0; i < word.length; i++) { if (me !== token) return; kw.textContent += word[i]; await wait(110); }
      typeEl.appendChild(doc.createTextNode(" "));
      await wait(260); if (me !== token) return;
      pop.hidden = false;
      await wait(2000); if (me !== token) return;
      pop.classList.add("closed");
      await wait(200); if (me !== token) return;
      pop.hidden = true; ghost.hidden = false; ghost.classList.add("in");
      if (tip) tip.classList.add("on");
      if (col) col.textContent = "Ln 3, Col 41";
    }
    var started = false;
    var io = new IntersectionObserver(function (entries) {
      if (!started && entries[0].isIntersecting) { started = true; io.disconnect(); play(); }
    }, { threshold: 0.6 });
    io.observe(fig);
    if (replay) replay.addEventListener("click", play);
  });

  /* ---------- key binding search (keybindings page) ---------- */
  var q = $("#q");
  if (q) {
    var group = $("[data-tabs]");
    var tabBtns = $$('[role="tab"]', group);
    var panelsEl = tabBtns.map(function (t) { return doc.getElementById(t.getAttribute("aria-controls")); });
    var countEl = $("#match-count");
    var empties = $$(".no-match");
    function run() {
      var term = q.value.trim().toLowerCase();
      var counts = panelsEl.map(function (panel) {
        var n = 0;
        $$("tr[data-row]", panel).forEach(function (tr) {
          var hit = !term || tr.textContent.toLowerCase().indexOf(term) >= 0;
          tr.hidden = !hit; if (hit) n++;
        });
        $$("section.block", panel).forEach(function (sec) {
          sec.hidden = !$$("tr[data-row]:not([hidden])", sec).length;
        });
        var none = $(".no-match", panel);
        if (none) none.classList.toggle("show", term !== "" && n === 0);
        return n;
      });
      tabBtns.forEach(function (t, i) {
        var c = $(".count", t);
        if (c) c.textContent = term ? String(counts[i]) : "";
      });
      if (countEl) countEl.textContent = term ? (function (n) { return n + (n === 1 ? " match" : " matches"); })(counts.reduce(function (a, b) { return a + b; }, 0)) : "";
      var cur = tabBtns.findIndex(function (t) { return t.getAttribute("aria-selected") === "true"; });
      if (term && counts[cur] === 0) {
        var alt = counts.findIndex(function (n) { return n > 0; });
        if (alt >= 0 && group._select) group._select(alt, false);
      }
    }
    q.addEventListener("input", run);
    q.addEventListener("keydown", function (e) { if (e.key === "Escape") { q.value = ""; run(); } });
    doc.addEventListener("keydown", function (e) {
      var t = e.target, typing = t && (/^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName) || t.isContentEditable);
      if (e.key === "/" && !typing && !e.ctrlKey && !e.metaKey && !e.altKey) { e.preventDefault(); q.focus(); q.select(); }
    });
  }
})();
