// KaizeNotch page: the pill, the panel and the timer.
// Rust (src-tauri) owns the files, the report, sharing, the window and the tray; this page owns what
// is on screen and the running session. Same behaviour as StudyNotch on the Mac:
//   tap a tile      start a pomodoro (activities with choices ask which one first: Pray -> Asar)
//   ✓ on a habit    "already did it": logs a finished session ending now, no timer
//   pencil          edit name, length, break, tab, choices, habit or time only
//   + after tabs    add your own category
"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// ---------- state ----------
let S = { subjects: [], categories: [], todayMinutes: {}, todayDone: [] };
const ui = {
  open: false, tab: "academic",
  adding: false, newName: "",
  addingCat: false, catName: "", catProblem: "",
  editing: null, edit: {},
  choosing: null, choosingTick: false,
  flash: null, problem: "",
  sharing: false, quitting: false,
};
let share = { available: false, state: "off" };   // from Rust: state off | working | on, url, updated, error
let run = null;    // { name, detail, category, planned, startedAt, endAt, remaining, paused }
let rest = null;   // { name, endAt, remaining, paused }
let collapseTimer = null;
let lastHeight = 0;

const MAX_NAME = 24;
const $ = (sel) => document.querySelector(sel);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const cat = (key) => S.categories.find((c) => c.key === key) || S.categories[0] || { key, label: key, color: "#888" };
const subjectNamed = (name) => S.subjects.find((s) => s.name === name);
const doneSet = () => new Set(S.todayDone);

const ICON = {
  play: '<svg viewBox="0 0 16 16"><path d="M4 2.5v11l9-5.5z" fill="currentColor"/></svg>',
  pause: '<svg viewBox="0 0 16 16"><path d="M4 2h3v12H4zM9 2h3v12H9z" fill="currentColor"/></svg>',
  stop: '<svg viewBox="0 0 16 16"><rect x="3" y="3" width="10" height="10" rx="2" fill="currentColor"/></svg>',
  skip: '<svg viewBox="0 0 16 16"><path d="M3 2.5v11l7-5.5zM11 2.5h2.5v11H11z" fill="currentColor"/></svg>',
  check: '<svg viewBox="0 0 16 16"><path d="M3 8.5l3.2 3.2L13 5" stroke="currentColor" stroke-width="2.2" fill="none" stroke-linecap="round" stroke-linejoin="round"/></svg>',
  pencil: '<svg viewBox="0 0 16 16"><path d="M2.5 13.5l.6-3 7.6-7.6 2.4 2.4-7.6 7.6zM9.6 4l2.4 2.4" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linejoin="round"/></svg>',
  close: '<svg viewBox="0 0 16 16"><path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>',
};

// ---------- helpers ----------
function clock(sec) {
  const s = Math.max(0, Math.ceil(sec));
  return String(Math.floor(s / 60)).padStart(2, "0") + ":" + String(s % 60).padStart(2, "0");
}
function timeLabel(min) {
  const m = min || 0;
  return m < 60 ? Math.round(m) + "m" : (m / 60).toFixed(1) + "h";
}
function todayTotal() {
  return Object.values(S.todayMinutes).reduce((a, b) => a + b, 0);
}
// How close the pomodoro is to its end: last 20% = soon, last minute (or 10%) = final
function urgency() {
  if (!run || run.paused) return "calm";
  const total = run.planned * 60;
  if (run.remaining <= Math.min(60, total * 0.1)) return "final";
  if (run.remaining <= total * 0.2) return "soon";
  return "calm";
}
// A habit tile counts as done today: every choice done (or any one, for sunnah-style), or its minutes reached
function doneToday(s) {
  const d = doneSet();
  if (s.options && s.options.length) {
    const n = s.options.filter((o) => d.has(s.name + "|" + o)).length;
    return s.needsAll ? n === s.options.length : n > 0;
  }
  return (S.todayMinutes[s.name] || 0) >= s.minutes;
}
function chime(high) {
  try {
    const ctx = new AudioContext();
    [0, 0.12].forEach((t, i) => {
      const o = ctx.createOscillator(), g = ctx.createGain();
      o.frequency.value = (high ? 880 : 660) * (i ? 1.5 : 1);
      g.gain.setValueAtTime(0.0001, ctx.currentTime + t);
      g.gain.exponentialRampToValueAtTime(0.18, ctx.currentTime + t + 0.02);
      g.gain.exponentialRampToValueAtTime(0.0001, ctx.currentTime + t + 0.5);
      o.connect(g).connect(ctx.destination);
      o.start(ctx.currentTime + t);
      o.stop(ctx.currentTime + t + 0.55);
    });
  } catch (_) { /* no audio device: stay quiet */ }
}
function flash(text, seconds = 6) {
  ui.flash = text;
  render();
  setTimeout(() => { if (ui.flash === text) { ui.flash = null; render(); } }, seconds * 1000);
}
async function call(cmd, args) {
  try {
    ui.problem = "";
    return await invoke(cmd, args);
  } catch (e) {
    ui.problem = String(e);
    render();
    throw e;
  }
}

// ---------- the timer ----------
function start(s, detail = null) {
  ui.choosing = null;
  if (run) stop(false);
  if (rest) endBreak(false);
  const planned = s.minutes;
  run = { name: s.name, detail, category: s.category, planned, startedAt: new Date(), endAt: Date.now() + planned * 60000, remaining: planned * 60, paused: false };
  render();
}

async function stop(completed) {
  if (!run) return;
  const r = run;
  run = null;
  const actual = completed ? r.planned : (r.planned * 60 - r.remaining) / 60;
  const label = r.detail ? `${r.name} · ${r.detail}` : r.name;
  render();
  S = await call("log_session", { session: {
    subject: r.name, category: r.category, detail: r.detail, plannedMin: r.planned, actualMin: Math.round(actual * 10) / 10,
    startedAt: r.startedAt.toISOString(), endedAt: new Date().toISOString(), completed,
  } });
  if (completed) {
    chime(false);
    const s = subjectNamed(r.name);
    const brk = s ? s.breakMinutes : 5;
    if (brk > 0) rest = { name: r.name, endAt: Date.now() + brk * 60000, remaining: brk * 60, paused: false };
    else flash("✓ " + label);
  }
  render();
}

function endBreak(finished) {
  if (!rest) return;
  rest = null;
  if (finished) { chime(true); flash("Break over: pick the next one", 8); }
  render();
}

function togglePause() {
  const t = run || rest;
  if (!t) return;
  if (t.paused) { t.endAt = Date.now() + t.remaining * 1000; t.paused = false; } else { t.paused = true; }
  render();
}

// ✓ on a habit: log a finished session of its full length, ending now
async function checkIn(s, detail = null) {
  ui.choosing = null;
  ui.choosingTick = false;
  const end = new Date();
  S = await call("log_session", { session: {
    subject: s.name, category: s.category, detail, plannedMin: s.minutes, actualMin: s.minutes,
    startedAt: new Date(end - s.minutes * 60000).toISOString(), endedAt: end.toISOString(), completed: true,
  } });
  chime(false);
  flash("✓ " + (detail ? `${s.name} · ${detail}` : s.name));
}

setInterval(() => {
  const t = run || rest;
  if (!t || t.paused) return;
  t.remaining = Math.max(0, (t.endAt - Date.now()) / 1000);
  if (t.remaining <= 0) { if (run) stop(true); else endBreak(true); return; }
  renderClockOnly();
}, 250);

// ---------- edits ----------
function typing() { return ui.adding || ui.addingCat || ui.editing !== null || ui.sharing || ui.quitting; }

async function saveSubjects(list) {
  S = await call("save_subjects", { subjects: list });
  render();
}

async function addSubject() {
  const name = ui.newName.trim().slice(0, MAX_NAME);
  ui.adding = false;
  ui.newName = "";
  if (name && !S.subjects.some((s) => s.name.toLowerCase() === name.toLowerCase())) {
    // options / needsAll / isHabit left out: Rust fills the defaults (Pray -> the five prayers)
    await saveSubjects([...S.subjects, { name, minutes: 25, breakMinutes: 5, category: ui.tab }]);
  } else render();
}

function beginEdit(s) {
  ui.adding = false;
  ui.editing = s.name;
  ui.edit = { name: s.name, minutes: s.minutes, breakMinutes: s.breakMinutes, category: s.category, options: (s.options || []).join(", "), isHabit: s.isHabit };
  render();
  focusFirstInput();
}

async function saveEdit() {
  const e = ui.edit, original = ui.editing;
  const list = S.subjects.map((s) => ({ ...s }));
  const s = list.find((x) => x.name === original);
  if (!s) return cancelEdit();
  const name = e.name.trim().slice(0, MAX_NAME);
  if (name && !list.some((x) => x !== s && x.name.toLowerCase() === name.toLowerCase())) s.name = name;
  s.minutes = Math.max(1, Math.min(180, Math.round(e.minutes)));
  s.breakMinutes = Math.max(0, Math.min(60, Math.round(e.breakMinutes)));
  s.category = e.category;
  s.isHabit = e.category !== "academic" && e.isHabit;
  const seen = new Set();
  s.options = e.options.split(",").map((o) => o.trim().slice(0, 14))
    .filter((o) => o && !seen.has(o.toLowerCase()) && seen.add(o.toLowerCase())).slice(0, 8);
  ui.tab = e.category;
  ui.editing = null;
  await saveSubjects(list);
}

function cancelEdit() { ui.editing = null; ui.adding = false; ui.addingCat = false; ui.sharing = false; ui.quitting = false; ui.catProblem = ""; render(); }

async function removeSubject(name) {
  ui.editing = null;
  await saveSubjects(S.subjects.filter((s) => s.name !== name));
}

async function addCategory() {
  try {
    S = await invoke("add_category", { name: ui.catName });
    const custom = S.categories.filter((c) => !c.builtIn);
    ui.tab = custom[custom.length - 1].key;
    ui.addingCat = false;
    ui.catName = "";
    ui.catProblem = "";
  } catch (e) {
    ui.catProblem = String(e);
  }
  render();
}

async function removeCategory(key) {
  try {
    S = await invoke("remove_category", { key });
    ui.tab = "academic";
  } catch (e) {
    ui.problem = String(e);
  }
  render();
}

// ---------- drawing ----------
function renderPill() {
  const pill = $("#pill");
  pill.className = urgency();
  if (run) {
    const c = cat(run.category).color;
    pill.innerHTML = `<span class="dot" style="background:${c}"></span><span class="name">${esc(run.detail || run.name)}</span><span class="clock">${clock(run.remaining)}</span>${run.paused ? '<span class="muted">paused</span>' : ""}`;
  } else if (rest) {
    pill.innerHTML = `<span class="dot" style="background:var(--blue)"></span><span class="muted">Break</span><span class="clock">${clock(rest.remaining)}</span>`;
  } else if (ui.flash) {
    pill.innerHTML = `<span class="flash">${esc(ui.flash)}</span>`;
  } else {
    pill.innerHTML = `<span class="dot idle"></span><span>KaizeNotch</span><span class="muted">${timeLabel(todayTotal())} today</span>`;
  }
}

function statusHTML() {
  if (run) {
    const u = urgency();
    return `<div class="status ${u}" style="--c:${cat(run.category).color}">
      <div class="label">${esc(run.name)}${run.detail ? ` <small>· ${esc(run.detail)}</small>` : ""}<br><small>${run.paused ? "Paused" : "Focus"}: ${run.planned} min</small></div>
      <div class="clock" data-clock>${clock(run.remaining)}</div>
      <button class="round" data-act="pause" title="${run.paused ? "Resume" : "Pause"}">${run.paused ? ICON.play : ICON.pause}</button>
      <button class="round" data-act="stop" title="Stop (saves the time so far)">${ICON.stop}</button>
    </div>`;
  }
  if (rest) {
    return `<div class="status rest">
      <div class="label">Break after ${esc(rest.name)}<br><small>Stand up, drink water</small></div>
      <div class="clock" data-clock>${clock(rest.remaining)}</div>
      <button class="round" data-act="pause" title="${rest.paused ? "Resume" : "Pause"}">${rest.paused ? ICON.play : ICON.pause}</button>
      <button class="round" data-act="skip" title="Skip the break">${ICON.skip}</button>
    </div>`;
  }
  return "";
}

function tabsHTML() {
  const tabs = S.categories.map((c) => {
    const on = c.key === ui.tab;
    const x = on && !c.builtIn ? `<span data-act="rmcat" data-key="${esc(c.key)}" title="Remove this tab">×</span>` : "";
    return `<button class="tab ${on ? "on" : ""}" style="--c:${esc(c.color)}" data-act="tab" data-key="${esc(c.key)}"><span class="swatch"></span>${esc(c.label)}${x}</button>`;
  }).join("");
  return `<div class="toprow"><div class="tabs">${tabs}<button class="tab add" data-act="addcat" title="Add a category">+</button></div>
    <button class="quit" data-act="askquit" title="Quit KaizeNotch">${ICON.close}</button></div>`;
}

function tileHTML(s) {
  const c = cat(s.category).color;
  const mins = S.todayMinutes[s.name] || 0;
  const pct = Math.min(100, (mins / s.minutes) * 100);
  const isRunning = run && run.name === s.name;
  const done = s.isHabit && doneToday(s);
  const tick = s.isHabit ? `<button class="${done ? "done" : ""}" data-act="tick" data-name="${esc(s.name)}" title="Already did it">${ICON.check}</button>` : "";
  return `<div class="tile ${isRunning ? "running" : ""}" role="button" tabindex="0" style="--c:${esc(c)}" data-act="tap" data-name="${esc(s.name)}">
    <span class="corner">${tick}<button data-act="edit" data-name="${esc(s.name)}" title="Edit">${ICON.pencil}</button></span>
    <span class="name">${esc(s.name)}</span>
    <span class="meta">${done ? '<span class="ok">✓</span>' : ""}${timeLabel(mins)} today · ${s.minutes}m${s.options && s.options.length ? ` · ${s.options.length} choices` : ""}</span>
    <span class="bar"><i style="width:${pct}%"></i></span>
  </div>`;
}

function shareHTML() {
  const err = share.error ? `<div class="problem">${esc(share.error)}</div>` : "";
  if (share.state === "working") {
    return `<div class="form"><div class="row"><span>Sharing</span><span style="color:var(--muted)">Working…</span></div></div>`;
  }
  if (share.state === "on") {
    return `<div class="form">
      <div class="row"><span>Your link</span><input type="text" id="shareUrl" readonly value="${esc(share.url)}"></div>
      <div class="row"><span></span><span style="color:var(--muted)">${share.updated ? "Updated " + esc(share.updated) : "First upload is on its way"} · anyone with the link can see your habits and totals</span></div>
      ${err}
      <div class="actions"><button class="pillbtn danger" data-act="sharestop">Stop sharing</button><span style="flex:1"></span>
        <button class="pillbtn" data-act="cancel">Close</button><button class="pillbtn" data-act="shareopen">Open</button><button class="pillbtn primary" data-act="sharecopy">Copy link</button></div>
    </div>`;
  }
  return `<div class="form">
    <div class="row"><span>Share</span><span>Put a read-only copy of your report on the web and get a link for your phone or a friend.</span></div>
    <div class="row"><span></span><span style="color:var(--muted)">Only habits, streaks and totals are uploaded, never your session times. No account needed. You can stop any time.</span></div>
    ${err}
    <div class="actions"><button class="pillbtn" data-act="cancel">Cancel</button><button class="pillbtn primary" data-act="sharestart">Create my link</button></div>
  </div>`;
}

function formHTML() {
  if (ui.quitting) {
    return `<div class="form">
      <div class="row"><span>Quit?</span><span>KaizeNotch closes and the pill disappears until you open it again.${run ? " The running session is saved first." : ""}</span></div>
      <div class="actions"><button class="pillbtn" data-act="cancel">Cancel</button><button class="pillbtn primary quitbtn" data-act="quit">Quit KaizeNotch</button></div>
    </div>`;
  }
  if (ui.sharing) return shareHTML();
  if (ui.addingCat) {
    return `<div class="form">
      <div class="row"><span>New tab</span><input type="text" id="catName" maxlength="16" placeholder="e.g. Quran Club" value="${esc(ui.catName)}"></div>
      ${ui.catProblem ? `<div class="problem">${esc(ui.catProblem)}</div>` : ""}
      <div class="actions"><button class="pillbtn" data-act="cancel">Cancel</button><button class="pillbtn primary" data-act="savecat">Add tab</button></div>
    </div>`;
  }
  if (ui.editing !== null) {
    const e = ui.edit;
    const opts = S.categories.map((c) => `<option value="${esc(c.key)}" ${c.key === e.category ? "selected" : ""}>${esc(c.label)}</option>`).join("");
    return `<div class="form">
      <div class="row"><span>Name</span><input type="text" id="editName" maxlength="${MAX_NAME}" value="${esc(e.name)}"></div>
      <div class="row"><span>Focus</span><span class="step"><button data-act="min-">−</button><b>${e.minutes} min</b><button data-act="min+">+</button></span>
        <span style="width:16px"></span><span style="color:var(--muted)">Break</span><span class="step"><button data-act="brk-">−</button><b>${e.breakMinutes} min</b><button data-act="brk+">+</button></span></div>
      <div class="row"><span>Tab</span><select id="editCat">${opts}</select></div>
      <div class="row"><span>Choices</span><input type="text" id="editOptions" placeholder="e.g. Subuh, Zohor, Asar (optional)" value="${esc(e.options)}"></div>
      ${e.category !== "academic" ? `<label class="row toggle"><span>Habit</span><input type="checkbox" id="editHabit" ${e.isHabit ? "checked" : ""}> counts toward a perfect day (off = time only)</label>` : ""}
      <div class="actions"><button class="pillbtn danger" data-act="delete">Delete</button><span style="flex:1"></span><button class="pillbtn" data-act="cancel">Cancel</button><button class="pillbtn primary" data-act="save">Save</button></div>
    </div>`;
  }
  if (ui.choosing) {
    const s = ui.choosing, d = doneSet();
    const buttons = s.options.map((o) => `<button class="${d.has(s.name + "|" + o) ? "done" : ""}" data-act="choose" data-opt="${esc(o)}">${esc(o)}</button>`).join("");
    return `<div class="form" style="--c:${esc(cat(s.category).color)}">
      <div class="row"><span>${esc(s.name)}</span><span style="color:var(--muted)">${ui.choosingTick ? "Which one did you do?" : "Which one now?"}</span></div>
      <div class="choices">${buttons}</div>
      <div class="actions"><button class="pillbtn" data-act="cancelchoose">Cancel</button></div>
    </div>`;
  }
  return "";
}

function renderPanel() {
  const list = S.subjects.filter((s) => s.category === ui.tab);
  const add = ui.adding
    ? `<div class="tile add" style="padding:8px"><input type="text" id="newName" maxlength="${MAX_NAME}" placeholder="Name, then Enter" value="${esc(ui.newName)}" style="width:100%;background:rgba(0,0,0,.35);border:1px solid var(--line);border-radius:8px;padding:5px 8px;font-size:12px;outline:none"></div>`
    : `<button class="tile add" data-act="add" title="Add an activity">+</button>`;
  const foot = ui.flash ? `<span class="flash">${esc(ui.flash)}</span>` : ui.problem ? `<span style="color:#ff8c85">${esc(ui.problem)}</span>` : `<span>${timeLabel(todayTotal())} today · v${esc(S.version || "?")}</span>`;
  $("#panel").innerHTML = `${statusHTML()}${tabsHTML()}${formHTML()}
    <div class="grid">${list.map(tileHTML).join("")}${add}</div>
    <div class="foot">${foot}<span class="links">${share.available ? `<button data-act="share">${share.state === "on" ? '<span class="live"></span>Sharing is on' : "Share on the web"}</button>` : ""}<button data-act="report">Open my report ↗</button></span></div>`;
}

function render() {
  $("#notch").classList.toggle("collapsed", !ui.open);
  renderPill();
  if (ui.open) {
    renderPanel();
    fit();
  }
}

// Only the numbers, every 250 ms, so typing and hovering are never interrupted
function renderClockOnly() {
  renderPill();
  const t = run || rest;
  const el = document.querySelector("[data-clock]");
  if (el && t) el.textContent = clock(t.remaining);
  const st = document.querySelector(".status");
  if (st && run) st.className = "status " + urgency();
}

// Ask Rust for a window as tall as the panel
function fit() {
  requestAnimationFrame(() => {
    const h = Math.ceil($("#panel").scrollHeight) + 2;
    if (h !== lastHeight) {
      lastHeight = h;
      invoke("set_expanded", { expanded: true, height: h });
    }
  });
}

function focusFirstInput() {
  requestAnimationFrame(() => {
    const el = document.querySelector("#panel input[type=text]");
    if (el) { invoke("focus_window"); el.focus(); el.setSelectionRange(el.value.length, el.value.length); }
  });
}

// ---------- open / close ----------
function setHover(inside) {
  clearTimeout(collapseTimer);
  if (inside) {
    if (!ui.open) { ui.open = true; lastHeight = 0; render(); }
    return;
  }
  // keep it open while typing or picking a choice, even if the mouse wanders off
  if (typing() || ui.choosing) return;
  collapseTimer = setTimeout(() => {
    ui.open = false;
    ui.problem = "";
    lastHeight = 0;
    invoke("set_expanded", { expanded: false });
    render();
  }, 350);
}
listen("hover", (e) => setHover(e.payload));
document.addEventListener("mouseenter", () => setHover(true));

// ---------- clicks and keys ----------
document.addEventListener("click", async (ev) => {
  const el = ev.target.closest("[data-act]");
  if (!el) return;
  ev.stopPropagation();
  const act = el.dataset.act, name = el.dataset.name, s = name ? subjectNamed(name) : null;
  switch (act) {
    case "tap":
      if (!s) return;
      if (s.options && s.options.length) { ui.choosing = s; ui.choosingTick = false; render(); } else start(s);
      break;
    case "tick":
      if (!s) return;
      if (s.options && s.options.length) { ui.choosing = s; ui.choosingTick = true; render(); } else await checkIn(s);
      break;
    case "choose": {
      const s2 = ui.choosing, opt = el.dataset.opt;
      if (ui.choosingTick) await checkIn(s2, opt); else start(s2, opt);
      break;
    }
    case "cancelchoose": ui.choosing = null; render(); break;
    case "edit": if (s) beginEdit(s); break;
    case "pause": togglePause(); break;
    case "stop": await stop(false); break;
    case "skip": endBreak(false); break;
    case "tab": ui.tab = el.dataset.key; ui.adding = false; render(); break;
    case "rmcat": await removeCategory(el.dataset.key); break;
    case "add": ui.editing = null; ui.adding = true; render(); focusFirstInput(); break;
    case "addcat": ui.addingCat = true; ui.catName = ""; ui.catProblem = ""; render(); focusFirstInput(); break;
    case "savecat": await addCategory(); break;
    case "cancel": cancelEdit(); break;
    case "save": await saveEdit(); break;
    case "delete": await removeSubject(ui.editing); break;
    case "min+": ui.edit.minutes = Math.min(180, (Math.floor(ui.edit.minutes / 5) + 1) * 5); render(); break;
    case "min-": ui.edit.minutes = Math.max(1, Math.floor((ui.edit.minutes - 1) / 5) * 5); render(); break;
    case "brk+": ui.edit.breakMinutes = Math.min(60, ui.edit.breakMinutes + 1); render(); break;
    case "brk-": ui.edit.breakMinutes = Math.max(0, ui.edit.breakMinutes - 1); render(); break;
    case "report": invoke("open_report"); break;
    case "askquit": cancelEdit(); ui.quitting = true; render(); break;
    case "quit": if (run) await stop(false); invoke("quit_app"); break;
    case "share":
      cancelEdit();
      ui.sharing = true;
      share = await invoke("share_info");
      render();
      break;
    case "sharestart":
      share = { ...share, state: "working", error: null };
      render();
      share = await invoke("share_start");
      render();
      break;
    case "sharestop":
      share = { ...share, state: "working", error: null };
      render();
      share = await invoke("share_stop");
      render();
      break;
    case "sharecopy": await invoke("share_copy"); flash("Link copied", 3); break;
    case "shareopen": invoke("share_open"); break;
  }
});

// Inputs keep their text in ui.* so a re-render never loses what was typed
document.addEventListener("input", (ev) => {
  const t = ev.target;
  if (t.id === "newName") ui.newName = t.value;
  if (t.id === "catName") ui.catName = t.value;
  if (t.id === "editName") ui.edit.name = t.value;
  if (t.id === "editOptions") ui.edit.options = t.value;
  if (t.id === "editHabit") ui.edit.isHabit = t.checked;
  if (t.id === "editCat") { ui.edit.category = t.value; render(); }
});
document.addEventListener("change", (ev) => {
  if (ev.target.id === "editHabit") ui.edit.isHabit = ev.target.checked;
});
document.addEventListener("keydown", async (ev) => {
  const id = ev.target.id;
  if (ev.key === "Escape") { ui.choosing = null; cancelEdit(); return; }
  if (ev.key !== "Enter") return;
  if (id === "newName") await addSubject();
  else if (id === "catName") await addCategory();
  else if (id === "editName" || id === "editOptions") await saveEdit();
});
document.addEventListener("contextmenu", (ev) => ev.preventDefault());

// ---------- go ----------
(async () => {
  S = await invoke("get_state");
  share = await invoke("share_info");
  render();
})();
