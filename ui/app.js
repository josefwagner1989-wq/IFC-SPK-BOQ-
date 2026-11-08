// IFC Sprinkler Viewer – interface
import * as THREE from "three";
import { OrbitControls } from "./vendor/OrbitControls.js";
import { mountTimmy, setTimmy } from "./timmy.js";

document.querySelectorAll("[data-timmy]").forEach((el) => mountTimmy(el, el.dataset.timmy));

const T = window.__TAURI__;
window.addEventListener("error", (e) => toast("Fehler: " + e.message, true, 10000));
window.addEventListener("unhandledrejection", (e) => toast("Fehler: " + (e.reason?.message || e.reason), true, 10000));
const invoke = (cmd, args) => T.core.invoke(cmd, args);
const $ = (s) => document.querySelector(s);
const $$ = (s) => [...document.querySelectorAll(s)];

let renderer, scene, camera, controls, pipeObj, fitObj, sprObj, accObj, hlLines, hlPts, grid;
let hemi, sun, ground;
let detail = "light";
let mainMeshes = [];              // malhas sólidas do modelo principal
const refs = new Map();           // id -> { name, meshes, visible }
const clipping = [];              // planos de corte ativos (comuns a todos os materiais)
const clipPlane = new THREE.Plane(new THREE.Vector3(0, 0, -1), 0);
const measures = [];              // { a, b, obj, label }
let measureMode = false, measureStart = null;

// ------------------------------------------------------------ Estado
const st = {
  path: null,
  summary: null,
  filter: { systems: new Set(), worksets: new Set(), storeys: new Set(), categories: new Set() },
  data: null,         // LoadResult
  qtoTab: "pipes",
  anlage: "Alle",
  selectedRow: null,
};

// ------------------------------------------------------------ Funções auxiliares
const fmtN = (v, d = 0) => (v ?? 0).toLocaleString("de-DE", { minimumFractionDigits: d, maximumFractionDigits: d });
const fmtSize = (b) => (b > 1e9 ? fmtN(b / 1e9, 2) + " GB" : fmtN(b / 1e6, 1) + " MB");
const esc = (s) => String(s ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
const baseName = (p) => p.split(/[\\/]/).pop();
function toast(msg, err = false, ms = 3500) {
  const t = $("#toast");
  t.textContent = msg;
  t.className = "toast" + (err ? " err" : "");
  t.hidden = false;
  clearTimeout(t._h);
  t._h = setTimeout(() => (t.hidden = true), ms);
}
function store(k, v) { try { localStorage.setItem(k, JSON.stringify(v)); } catch {} }
function load(k, d) { try { const v = localStorage.getItem(k); return v ? JSON.parse(v) : d; } catch { return d; } }

function show(view) {
  $$(".view").forEach((v) => v.classList.toggle("active", v.id === view));
  const main = view !== "viewStart";
  $("#btnNew").hidden = !main;
  $("#btnFilter").hidden = view !== "viewMain";
  $("#btnExportTop").hidden = view !== "viewMain";
  $("#fileInfo").hidden = !main;
  if (view === "viewMain") requestAnimationFrame(resize3d);
}

// ------------------------------------------------------------ Tema
function setTheme(t) {
  document.documentElement.dataset.theme = t;
  store("theme", t);
  if (scene) applySceneTheme();
}
$("#themeToggle").onclick = () => setTheme(document.documentElement.dataset.theme === "dark" ? "light" : "dark");
setTheme(load("theme", window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light"));

// ------------------------------------------------------------ Início
function renderRecent() {
  const list = load("recent", []);
  const ul = $("#recentList");
  if (!list.length) { ul.innerHTML = '<li class="muted">Noch keine Dateien</li>'; return; }
  ul.innerHTML = list.map((r, i) => `<li class="file" data-i="${i}"><span>${esc(baseName(r.path))}</span><span class="rmeta">${fmtSize(r.size)} · ${new Date(r.date).toLocaleDateString("de-DE")}</span></li>`).join("");
  ul.querySelectorAll("li.file").forEach((li) => (li.onclick = () => openFile(list[+li.dataset.i].path)));
}
function addRecent(path, size) {
  const list = load("recent", []).filter((r) => r.path !== path);
  list.unshift({ path, size, date: Date.now() });
  store("recent", list.slice(0, 8));
}
renderRecent();

$("#btnOpen").onclick = async () => {
  const p = await T.dialog.open({ multiple: false, filters: [{ name: "IFC-Modell", extensions: ["ifc", "IFC", "ifczip", "IFCZIP", "rvt", "RVT"] }] });
  if (p) openFile(typeof p === "string" ? p : p.path);
};
$("#btnNew").onclick = () => { show("viewStart"); renderRecent(); setTimmy($("#timmyStart"), "happy", "wave", 1800); };
$("#btnFilter").onclick = () => show("viewFilter");

// Arrastar e soltar (o Tauri fornece os caminhos dos arquivos)
try {
  T.webview.getCurrentWebview().onDragDropEvent((ev) => {
    const dz = $("#dropzone");
    const p = ev.payload;
    if (p.type === "over" || p.type === "enter") dz.classList.add("over");
    else if (p.type === "leave") dz.classList.remove("over");
    else if (p.type === "drop") {
      dz.classList.remove("over");
      const f = (p.paths || []).find((x) => /\.(ifc|ifczip|rvt)$/i.test(x));
      if (f) openFile(f);
      else toast("Bitte eine .ifc- oder .ifczip-Datei ablegen.", true);
    }
  });
} catch (e) { console.warn(e); }

// ------------------------------------------------------------ Progresso
T.event.listen("progress", (ev) => {
  const { phase, done, total } = ev.payload;
  const pct = total ? (done * 100) / total : 0;
  $("#progBar").style.width = pct.toFixed(1) + "%";
  $("#progText").textContent = `${phase === "scan" ? "Scan" : "Geometrie"} ${fmtN(pct, 0)} % · ${fmtSize(done)} von ${fmtSize(total)}`;
});
$("#btnCancel").onclick = () => invoke("cancel");

// ------------------------------------------------------------ Varredura
async function openFile(path) {
  st.path = path;
  $("#fileInfo").textContent = path;
  show("viewFilter");
  $("#filterGrid").hidden = true;
  $("#filterFoot").hidden = true;
  $("#scanCard").hidden = false;
  $("#scanTitle").textContent = "Datei wird gescannt …";
  $("#scanStats").innerHTML = "";
  $("#progBar").style.width = "0";
  $("#btnCancel").hidden = false;
  setTimmy($("#timmyScan"), "swim");
  try {
    const res = await invoke("scan", { path });
    setTimmy($("#timmyScan"), "happy", "idle", 2000);
    st.summary = res.summary;
    addRecent(path, res.summary.file_size);
    const s = res.summary;
    $("#scanTitle").textContent = `${baseName(path)} – ${fmtN(s.elements)} Rohrleitungsbauteile gefunden`;
    $("#progBar").style.width = "100%";
    $("#progText").textContent = `Scan fertig in ${fmtN(s.seconds, 1)} s`;
    $("#btnCancel").hidden = true;
    $("#scanStats").innerHTML = `<span>Größe <b>${fmtSize(s.file_size)}</b></span><span>Schema <b>${esc(s.schema)}</b></span><span>Einheit <b>${s.unit_scale_m === 0.001 ? "mm" : s.unit_scale_m === 1 ? "m" : s.unit_scale_m + " m"}</b></span><span>Entitäten <b>${fmtN(s.entities_indexed + s.entities_skipped)}</b></span>`;
    // Pré-preenche os filtros
    const f = st.filter;
    f.systems = new Set(res.default_filter.systems.length ? res.default_filter.systems : s.systems.map((x) => x.name));
    f.worksets = new Set(res.default_filter.worksets.length ? res.default_filter.worksets : s.worksets.map((x) => x.name));
    f.storeys = new Set(s.storeys.map((x) => x.name));
    f.categories = new Set(s.categories.map((x) => x.name));
    renderFilters();
    $("#filterGrid").hidden = false;
    $("#filterFoot").hidden = false;
    if (!s.elements) { setTimmy($("#timmyScan"), "sad"); }
    if (!s.elements) toast("Keine Rohre, Formstücke oder Armaturen im Modell gefunden.", true, 6000);
  } catch (e) {
    $("#scanTitle").textContent = "Datei konnte nicht gelesen werden";
    setTimmy($("#timmyScan"), "sad");
    $("#progText").textContent = String(e);
    $("#btnCancel").hidden = true;
    toast(String(e), true, 8000);
  }
}

function renderFilters() {
  const s = st.summary;
  const groups = { systems: s.systems, worksets: s.worksets, storeys: s.storeys, categories: s.categories };
  for (const [k, items] of Object.entries(groups)) {
    const el = $("#f_" + k);
    const q = ($(`[data-search="${k}"]`)?.value || "").toLowerCase();
    el.innerHTML = items
      .filter((it) => !q || it.name.toLowerCase().includes(q))
      .map((it) => `<label class="fitem"><input type="checkbox" data-k="${k}" value="${esc(it.name)}" ${st.filter[k].has(it.name) ? "checked" : ""}><span class="nm">${esc(it.name)}</span>${it.sprinkler_like ? '<span class="tag">Brandschutz</span>' : ""}<span class="ct">${fmtN(it.count)}</span></label>`)
      .join("") || '<p class="muted">–</p>';
    el.querySelectorAll("input").forEach((cb) => (cb.onchange = () => { cb.checked ? st.filter[k].add(cb.value) : st.filter[k].delete(cb.value); updateSelCount(); }));
  }
  updateSelCount();
}
$$("[data-search]").forEach((i) => (i.oninput = renderFilters));
$$("[data-all]").forEach((a) => (a.onclick = () => { const k = a.dataset.all; st.filter[k] = new Set(st.summary[k].map((x) => x.name)); renderFilters(); }));
$$("[data-none]").forEach((a) => (a.onclick = () => { st.filter[a.dataset.none] = new Set(); renderFilters(); }));
function updateSelCount() {
  const f = st.filter;
  const empty = ["systems", "worksets", "storeys", "categories"].filter((k) => st.summary[k].length && !f[k].size);
  $("#btnLoad").disabled = empty.length > 0;
  $("#selCount").textContent = empty.length ? "Bitte in jeder Gruppe mindestens einen Eintrag wählen." : "";
}

$("#btnLoad").onclick = async () => {
  const s = st.summary;
  // Tudo selecionado = sem filtro (mais rápido, inclui atribuição múltipla)
  const pick = (k) => (st.filter[k].size === s[k].length ? [] : [...st.filter[k]]);
  const filter = { systems: pick("systems"), worksets: pick("worksets"), storeys: pick("storeys"), categories: pick("categories") };
  $("#btnLoad").disabled = true;
  $("#scanTitle").textContent = "Auswahl wird geladen …";
  $("#progBar").style.width = "0";
  $("#btnCancel").hidden = false;
  setTimmy($("#timmyScan"), "swim");
  try {
    st.data = await invoke("load", { filter });
    $("#btnCancel").hidden = true;
    $("#progText").textContent = `${fmtN(st.data.count)} Bauteile geladen in ${fmtN(st.data.seconds, 1)} s`;
    show("viewMain");
    build3d();
    const present = st.data.anlagen.filter((a) => a.count > 0);
    st.anlage = present.length === 1 ? present[0].name : "Alle";
    if (st.anlage !== "Alle") st.data.takeoff = await invoke("takeoff", { anlage: st.anlage });
    renderAnlagen();
    renderQto();
    setTab("tab3d");
  } catch (e) {
    toast(String(e), true, 8000);
    setTimmy($("#timmyScan"), "sad");
    $("#btnCancel").hidden = true;
  } finally {
    $("#btnLoad").disabled = false;
  }
};

// ------------------------------------------------------------ Abas
function setTab(id) {
  $$("#mainTabs .tab").forEach((b) => b.classList.toggle("active", b.dataset.tab === id));
  $$(".tabpane").forEach((p) => p.classList.toggle("active", p.id === id));
  if (id === "tab3d") requestAnimationFrame(resize3d);
}
$$("#mainTabs .tab").forEach((b) => (b.onclick = () => setTab(b.dataset.tab)));

// ------------------------------------------------------------ 3D
const DN_COLORS = [
  [20, "#7dd3fc"], [25, "#38bdf8"], [32, "#22c55e"], [40, "#a3e635"], [50, "#f59e0b"], [65, "#fb923c"],
  [80, "#ef4444"], [100, "#e11d48"], [125, "#c026d3"], [150, "#8b5cf6"], [200, "#6366f1"], [1e9, "#4338ca"],
];
const dnNum = (lbl) => { const m = /DN\s?(\d+)/.exec(lbl || ""); return m ? +m[1] : null; };
const dnColor = (lbl) => { const d = dnNum(lbl); if (d == null) return "#94a3b8"; return DN_COLORS.find(([x]) => d <= x)[1]; };
let dotTex;
function dotTexture() {
  if (dotTex) return dotTex;
  const c = document.createElement("canvas"); c.width = c.height = 64;
  const g = c.getContext("2d"); g.beginPath(); g.arc(32, 32, 28, 0, Math.PI * 2); g.fillStyle = "#fff"; g.fill();
  g.lineWidth = 6; g.strokeStyle = "rgba(0,0,0,.35)"; g.stroke();
  dotTex = new THREE.CanvasTexture(c); return dotTex;
}

function init3d() {
  if (renderer) return;
  const canvas = $("#canvas");
  renderer = new THREE.WebGLRenderer({ canvas, antialias: true });
  renderer.setPixelRatio(window.devicePixelRatio);
  scene = new THREE.Scene();
  camera = new THREE.PerspectiveCamera(45, 1, 0.05, 20000);
  camera.up.set(0, 0, 1);
  controls = new OrbitControls(camera, canvas);
  controls.enableDamping = true;
  controls.screenSpacePanning = true;
  renderer.localClippingEnabled = true;
  renderer.shadowMap.type = THREE.PCFSoftShadowMap;
  hemi = new THREE.HemisphereLight(0xffffff, 0x8c99aa, 1.25);
  hemi.position.set(0, 0, 1);
  sun = new THREE.DirectionalLight(0xffffff, 1.6);
  scene.add(hemi, sun, sun.target);
  applySceneTheme();
  new ResizeObserver(resize3d).observe($("#viewport"));
  canvas.addEventListener("pointerdown", (e) => (canvas._down = [e.clientX, e.clientY]));
  canvas.addEventListener("pointerup", (e) => {
    const d = canvas._down; if (!d || Math.hypot(e.clientX - d[0], e.clientY - d[1]) > 4) return;
    if (measureMode) measureClick(e); else pick(e);
  });
  (function loop() { requestAnimationFrame(loop); controls.update(); renderer.render(scene, camera); updateLabels(); })();
}
function applySceneTheme() {
  const dark = document.documentElement.dataset.theme === "dark";
  scene.background = new THREE.Color(dark ? "#0a1422" : "#f7fafd");
  if (grid) grid.material.color = new THREE.Color(dark ? "#1d324f" : "#d5deea");
}
function resize3d() {
  if (!renderer) return;
  const vp = $("#viewport");
  const w = vp.clientWidth, h = vp.clientHeight;
  if (!w || !h) return;
  renderer.setSize(w, h, false);
  camera.aspect = w / h;
  camera.updateProjectionMatrix();
}

function clearObj(o) { if (o) { scene.remove(o); o.geometry.dispose(); o.material.dispose(); } }

function build3d() {
  init3d();
  [pipeObj, fitObj, sprObj, accObj, hlLines, hlPts, grid, ground].forEach(clearObj);
  hlLines = hlPts = ground = null;
  mainMeshes.forEach(clearObj); mainMeshes = [];
  for (const [id, r] of refs) { r.meshes.forEach(clearObj); invoke("remove_reference", { id }).catch(() => {}); }
  refs.clear(); renderRefs();
  clearMeasures(); setSection(null);
  const g = st.data.geometry, els = st.data.elements;
  // Tubos: linhas, cor conforme o DN
  const lp = new Float32Array(g.lines), lc = new Float32Array(g.lines.length);
  const col = new THREE.Color();
  for (let s = 0; s < g.line_elem.length; s++) {
    col.set(dnColor(els[g.line_elem[s]].dn));
    for (let k = 0; k < 2; k++) lc.set([col.r, col.g, col.b], s * 6 + k * 3);
  }
  const lg = new THREE.BufferGeometry();
  lg.setAttribute("position", new THREE.BufferAttribute(lp, 3));
  lg.setAttribute("color", new THREE.BufferAttribute(lc, 3));
  pipeObj = new THREE.LineSegments(lg, new THREE.LineBasicMaterial({ vertexColors: true, clippingPlanes: clipping }));
  pipeObj.userData.map = g.line_elem;
  scene.add(pipeObj);
  // Pontos: divididos em conexão / sprinkler / outros acessórios
  const parts = { fit: [], spr: [], acc: [] };
  for (let i = 0; i < g.point_elem.length; i++) {
    const e = els[g.point_elem[i]];
    const key = e.cat === 1 ? "fit" : e.kind === "Sprinkler" ? "spr" : e.cat === 2 ? "acc" : "fit";
    parts[key].push(i);
  }
  const mk = (idx, color, size) => {
    const pos = new Float32Array(idx.length * 3), map = new Uint32Array(idx.length);
    idx.forEach((i, j) => { pos.set(g.points.slice(i * 3, i * 3 + 3), j * 3); map[j] = g.point_elem[i]; });
    const geo = new THREE.BufferGeometry();
    geo.setAttribute("position", new THREE.BufferAttribute(pos, 3));
    const o = new THREE.Points(geo, new THREE.PointsMaterial({ color, size, sizeAttenuation: false, map: dotTexture(), transparent: true, alphaTest: 0.4, clippingPlanes: clipping }));
    o.userData.map = map;
    scene.add(o);
    return o;
  };
  fitObj = mk(parts.fit, "#94a3b8", 6);
  sprObj = mk(parts.spr, "#e53935", 9);
  accObj = mk(parts.acc, "#facc15", 10);
  // Grade abaixo do modelo
  const mn = g.bbox_min, mx = g.bbox_max;
  const size = Math.max(mx[0] - mn[0], mx[1] - mn[1], 1);
  grid = new THREE.GridHelper(size * 1.3, 26);
  grid.rotation.x = Math.PI / 2;
  grid.position.set((mn[0] + mx[0]) / 2, (mn[1] + mx[1]) / 2, mn[2] - 0.05);
  scene.add(grid);
  applySceneTheme();
  placeSun();
  setDetail("light", true);
  updateVisibility();
  renderLegend();
  fitView();
  $("#vpInfo").textContent = `${fmtN(g.line_elem.length)} Rohrachsen · ${fmtN(g.point_elem.length)} Punkte`;
  $("#props").innerHTML = '<h3>Eigenschaften</h3><p class="muted">Bauteil anklicken</p>';
}
let renderLegend = function () {
  const dns = [...new Set(st.data.takeoff.pipes.map((r) => r.dn))];
  $("#legend").innerHTML = dns.map((d) => `<div><i class="sw" style="background:${dnColor(d)}"></i>${esc(d)}</div>`).join("") || '<span class="muted">–</span>';
}
function fitView() {
  const g = st.data.geometry, mn = g.bbox_min, mx = g.bbox_max;
  const c = new THREE.Vector3((mn[0] + mx[0]) / 2, (mn[1] + mx[1]) / 2, (mn[2] + mx[2]) / 2);
  const r = Math.max(new THREE.Vector3(mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]).length() / 2, 1);
  controls.target.copy(c);
  const fov = THREE.MathUtils.degToRad(camera.fov);
  const fitH = r / Math.sin(fov / 2), fitW = r / Math.sin(Math.atan(Math.tan(fov / 2) * camera.aspect));
  const dist = Math.max(fitH, fitW) * 1.05;
  camera.position.copy(c).add(new THREE.Vector3(-0.8, -1.2, 1.0).normalize().multiplyScalar(dist));
  camera.near = dist / 1000; camera.far = dist * 20; camera.updateProjectionMatrix();
}
$("#btnFit").onclick = fitView;
function updateVisibility() {
  const light = detail === "light";
  if (pipeObj) pipeObj.visible = light && $("#showPipes").checked;
  if (fitObj) fitObj.visible = light && $("#showFittings").checked;
  if (sprObj) sprObj.visible = light && $("#showAcc").checked;
  if (accObj) accObj.visible = light && $("#showAcc").checked;
  if (grid) grid.visible = light;
  for (const m of mainMeshes) {
    const k = m.userData.cls % 4;
    m.visible = !light && (k === 0 ? $("#showPipes").checked : k === 1 ? $("#showFittings").checked : $("#showAcc").checked);
  }
}
["#showPipes", "#showFittings", "#showAcc"].forEach((s) => ($(s).onchange = updateVisibility));

function pick(e) {
  const mh = meshHit(e);
  if (mh) {
    if (mh.obj.userData.ref) { highlight([]); showRefDetails(mh.obj.userData.ref, mh.elem); return; }
    highlight([mh.elem]); showDetails(mh.elem); return;
  }
  return pickLines(e);
}
function pickLines(e) {
  // Seleção em pixels de tela: componente mais próximo dentro de 10 px
  const rect = renderer.domElement.getBoundingClientRect();
  const mx = e.clientX - rect.left, my = e.clientY - rect.top, W = rect.width, H = rect.height;
  const g = st.data.geometry, els = st.data.elements;
  const v = new THREE.Vector3();
  const proj = (x, y, z) => { v.set(x, y, z).project(camera); return v.z > 1 || v.z < -1 ? null : [(v.x + 1) / 2 * W, (1 - v.y) / 2 * H, v.z]; };
  let best = null, bestD = 10;
  const showP = $("#showFittings").checked, showA = $("#showAcc").checked;
  for (let i = 0; i < g.point_elem.length; i++) {
    const el = els[g.point_elem[i]];
    if (el.cat === 1 ? !showP : !showA) continue;
    const p = proj(g.points[i * 3], g.points[i * 3 + 1], g.points[i * 3 + 2]); if (!p) continue;
    const d = Math.hypot(p[0] - mx, p[1] - my);
    if (d < bestD - 2) { bestD = d; best = g.point_elem[i]; }   // pontos levemente preferidos
  }
  if ($("#showPipes").checked) {
    for (let s2 = 0; s2 < g.line_elem.length; s2++) {
      const o = s2 * 6;
      const a = proj(g.lines[o], g.lines[o + 1], g.lines[o + 2]), b = proj(g.lines[o + 3], g.lines[o + 4], g.lines[o + 5]);
      if (!a || !b) continue;
      const dx = b[0] - a[0], dy = b[1] - a[1], L2 = dx * dx + dy * dy;
      const t = L2 ? Math.max(0, Math.min(1, ((mx - a[0]) * dx + (my - a[1]) * dy) / L2)) : 0;
      const d = Math.hypot(a[0] + t * dx - mx, a[1] + t * dy - my);
      if (d < bestD) { bestD = d; best = g.line_elem[s2]; }
    }
  }
  if (best == null) { highlight([]); return; }
  highlight([best]);
  showDetails(best);
}

function highlight(ids) {
  clearObj(hlLines); clearObj(hlPts); hlLines = hlPts = null;
  if (!ids.length) return;
  const set = new Set(ids), g = st.data.geometry;
  const lines = [], pts = [];
  for (let s = 0; s < g.line_elem.length; s++) if (set.has(g.line_elem[s])) lines.push(...g.lines.slice(s * 6, s * 6 + 6));
  for (let i = 0; i < g.point_elem.length; i++) if (set.has(g.point_elem[i])) pts.push(...g.points.slice(i * 3, i * 3 + 3));
  const hc = document.documentElement.dataset.theme === "dark" ? "#ffffff" : "#0b2545";
  if (lines.length) {
    const geo = new THREE.BufferGeometry(); geo.setAttribute("position", new THREE.Float32BufferAttribute(lines, 3));
    hlLines = new THREE.LineSegments(geo, new THREE.LineBasicMaterial({ color: hc, depthTest: false }));
    hlLines.renderOrder = 2; scene.add(hlLines);
  }
  if (pts.length) {
    const geo = new THREE.BufferGeometry(); geo.setAttribute("position", new THREE.Float32BufferAttribute(pts, 3));
    hlPts = new THREE.Points(geo, new THREE.PointsMaterial({ color: hc, size: 13, sizeAttenuation: false, map: dotTexture(), transparent: true, alphaTest: 0.4, depthTest: false }));
    hlPts.renderOrder = 3; scene.add(hlPts);
  }
}

async function showDetails(idx) {
  try {
    const d = await invoke("details", { index: idx });
    const e = d.element;
    const rows = [
      ["Art", e.kind], ["Name", e.name], ["Typ", e.type_name], ["Nennweite", e.dn_label || "–"],
      ["Länge", e.length_m != null ? fmtN(e.length_m, 2) + " m" : "–"], ["Material", e.material || "–"],
      ["System", e.systems.join(", ") || "–"], ["Workset", e.worksets.join(", ") || "–"],
      ["Geschoss", e.storey || "–"], ["IFC-Klasse", e.class], ["GlobalId", e.guid],
    ];
    let html = `<h3>Eigenschaften</h3><div class="big">${esc(e.kind)} ${esc(e.dn_label)}</div><table>${rows.map(([k, v]) => `<tr><td>${k}</td><td>${esc(v)}</td></tr>`).join("")}</table>`;
    for (const [ps, props] of d.psets) {
      html += `<div class="psn">${esc(ps)}</div><table>${props.map(([k, v]) => `<tr><td>${esc(k)}</td><td>${esc(v)}</td></tr>`).join("")}</table>`;
    }
    $("#props").innerHTML = html;
  } catch (err) { toast(String(err), true); }
}

// ------------------------------------------------------------ Visualização (Alto / referências / corte / medição)
const MAIN_COLORS = {
  0: "#d32f2f", 1: "#8e1b1b", 2: "#ffb300", 3: "#607d8b",     // Sprinklers: tubo, conexão, sprinkler, acessório
  4: "#1e88e5", 5: "#0d47a1", 6: "#ffb300", 7: "#546e7a",     // Hidrantes
  8: "#f57c00", 9: "#b34700", 10: "#ffb300", 11: "#5d4037",   // Coluna seca
  12: "#9e9e9e", 13: "#6d6d6d", 14: "#ffb300", 15: "#616161", // não atribuído
};
const MAIN_LABEL = ["Rohre", "Formstücke", "Sprinkler", "Armaturen"];
const ANL_NAMES = ["Sprinkler", "Hydranten", "Trockensteigleitung", "Nicht zugeordnet"];
const REF_COLORS = { 0: "#e6dfd2", 1: "#c9cfd6", 2: "#9fd3ff", 3: "#a1887f", 4: "#aab3bd", 5: "#cbc3b5", 6: "#b7c6d8", 7: "#d4d4d4" };
const REF_LABEL = ["Wände", "Decken/Dächer", "Fenster/Fassade", "Türen", "Tragwerk", "Treppen/Geländer", "TGA", "Sonstiges"];

function parseGroups(buf) {
  const dv = new DataView(buf);
  let o = 0;
  const n = dv.getUint32(o, true); o += 4;
  const out = [];
  for (let g = 0; g < n; g++) {
    const cls = dv.getUint32(o, true), np = dv.getUint32(o + 4, true), ni = dv.getUint32(o + 8, true), nr = dv.getUint32(o + 12, true); o += 16;
    const pos = new Float32Array(buf.slice(o, o + np * 4)); o += np * 4;
    const idx = new Uint32Array(buf.slice(o, o + ni * 4)); o += ni * 4;
    const ranges = new Uint32Array(buf.slice(o, o + nr * 8)); o += nr * 8;
    out.push({ cls, pos, idx, ranges });
  }
  return out;
}
function meshFrom(g, color, opts = {}) {
  const geo = new THREE.BufferGeometry();
  geo.setAttribute("position", new THREE.BufferAttribute(g.pos, 3));
  geo.setIndex(new THREE.BufferAttribute(g.idx, 1));
  geo.computeBoundingSphere();
  const mat = new THREE.MeshStandardMaterial({
    color, flatShading: true, side: THREE.DoubleSide, roughness: 0.62, metalness: 0.08,
    transparent: !!opts.transparent, opacity: opts.transparent ? 0.32 : 1, depthWrite: !opts.transparent,
    clippingPlanes: clipping,
  });
  const m = new THREE.Mesh(geo, mat);
  m.userData = { cls: g.cls, ranges: g.ranges, ...opts.data };
  m.castShadow = m.receiveShadow = $("#renderShadows").checked;
  return m;
}
function elemOfTri(ranges, tri) {
  let lo = 0, hi = ranges.length / 2 - 1, r = 0;
  while (lo <= hi) { const mid = (lo + hi) >> 1; if (ranges[mid * 2] <= tri) { r = mid; lo = mid + 1; } else hi = mid - 1; }
  return ranges[r * 2 + 1];
}
function busy(on, text) {
  $("#vpBusy").hidden = !on;
  if (text) $("#vpBusyText").textContent = text;
  setTimmy($("#timmyBusy"), on ? "swim" : "idle");
}
T.event.listen("progress", (ev) => {
  const { phase, done, total } = ev.payload;
  if (phase === "mesh" || phase === "ref") $("#vpBusyText").textContent = `${phase === "ref" ? "Referenzmodell" : "Solide Darstellung"} wird geladen … ${fmtN(total ? done * 100 / total : 0)} %`;
});

async function setDetail(d, silent) {
  if (d === "high" && !mainMeshes.length && st.data) {
    busy(true, "Solide Darstellung wird erzeugt …");
    try {
      const buf = await invoke("mesh_main", { segs: 12 });
      for (const g of parseGroups(buf)) {
        if (!g.idx.length) continue;
        const m = meshFrom(g, MAIN_COLORS[g.cls] || "#999");
        scene.add(m); mainMeshes.push(m);
      }
    } catch (e) { toast(String(e), true, 8000); busy(false); return; }
    busy(false);
  }
  detail = d;
  $$("#detailSeg button").forEach((b) => b.classList.toggle("active", b.dataset.d === d));
  updateVisibility();
  renderLegend();
  if (!silent && d === "high" && !mainMeshes.length) toast("Für die Auswahl konnte keine solide Geometrie erzeugt werden.", true);
}
$$("#detailSeg button").forEach((b) => (b.onclick = () => setDetail(b.dataset.d)));

// Legenda por nível de detalhe
const _renderLegendDN = renderLegend;
renderLegend = function () {
  $("#legendTitle").textContent = detail === "high" ? "Farben" : "Nennweiten";
  $("#legendFixed").hidden = detail === "high";
  if (detail !== "high") return _renderLegendDN();
  const present = [...new Set(mainMeshes.map((m) => m.userData.cls))].sort((a, b) => a - b);
  $("#legend").innerHTML = present.map((c) => `<div><i class="sw" style="background:${MAIN_COLORS[c]}"></i>${ANL_NAMES[c >> 2]} · ${MAIN_LABEL[c % 4]}</div>`).join("");
};

// Sombras (teste de renderização)
function placeSun() {
  if (!st.data || !sun) return;
  const g = st.data.geometry, mn = g.bbox_min, mx = g.bbox_max;
  const c = new THREE.Vector3((mn[0] + mx[0]) / 2, (mn[1] + mx[1]) / 2, (mn[2] + mx[2]) / 2);
  const r = Math.max(new THREE.Vector3(mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]).length() / 2, 1);
  sun.position.copy(c).add(new THREE.Vector3(-0.5, -0.8, 1.4).normalize().multiplyScalar(r * 2));
  sun.target.position.copy(c);
  const sc = sun.shadow.camera;
  sc.left = sc.bottom = -r * 1.2; sc.right = sc.top = r * 1.2; sc.near = 0.1; sc.far = r * 5; sc.updateProjectionMatrix();
  sun.shadow.mapSize.set(2048, 2048);
  sun.shadow.bias = -0.0005;
}
$("#renderShadows").onchange = () => {
  const on = $("#renderShadows").checked;
  renderer.shadowMap.enabled = on;
  sun.castShadow = on;
  const all = [...mainMeshes, ...[...refs.values()].flatMap((r) => r.meshes)];
  all.forEach((m) => { m.castShadow = m.receiveShadow = on; });
  if (on && !ground && st.data) {
    const g = st.data.geometry, mn = g.bbox_min, mx = g.bbox_max;
    const size = Math.max(mx[0] - mn[0], mx[1] - mn[1], 1) * 3;
    ground = new THREE.Mesh(new THREE.PlaneGeometry(size, size), new THREE.ShadowMaterial({ opacity: 0.22, clippingPlanes: clipping }));
    ground.position.set((mn[0] + mx[0]) / 2, (mn[1] + mx[1]) / 2, mn[2] - 0.02);
    ground.receiveShadow = true;
    scene.add(ground);
  }
  if (ground) ground.visible = on;
  scene.traverse((o) => { if (o.material) o.material.needsUpdate = true; });
  if (on) toast("Render-Test: Schatten sind experimentell und können große Modelle deutlich verlangsamen.", false, 5000);
};

// Modelos de referência
$("#tRef").onclick = async () => {
  if (!st.data) return;
  const p = await T.dialog.open({ multiple: false, filters: [{ name: "IFC-Modell", extensions: ["ifc", "IFC", "ifczip", "IFCZIP"] }] });
  if (!p) return;
  const path = typeof p === "string" ? p : p.path;
  busy(true, "Referenzmodell wird geladen …");
  try {
    const info = await invoke("add_reference", { path, segs: 16 });
    const buf = await invoke("ref_mesh", { id: info.id });
    const meshes = [];
    const transp = $("#refTransp").checked;
    for (const g of parseGroups(buf)) {
      if (!g.idx.length) continue;
      const m = meshFrom(g, REF_COLORS[g.cls] || "#ccc", { transparent: transp || g.cls === 2, data: { ref: info.id } });
      m.renderOrder = -1;
      scene.add(m); meshes.push(m);
    }
    refs.set(info.id, { name: info.name, meshes, visible: true, count: info.elements });
    renderRefs();
    toast(`Referenz „${info.name}“ geladen (${fmtN(info.elements)} Bauteile) – nur Ansicht, kein Mengenauszug.`, false, 5000);
  } catch (e) { toast(String(e), true, 8000); }
  busy(false);
};
function renderRefs() {
  const el = $("#refList");
  if (!refs.size) { el.innerHTML = '<span class="muted small">Keine – über „＋ Referenz-IFC“ laden (nur Ansicht, kein Mengenauszug)</span>'; return; }
  el.innerHTML = [...refs].map(([id, r]) => `<div class="refItem"><input type="checkbox" data-v="${id}" ${r.visible ? "checked" : ""}><span class="nm" title="${esc(r.name)}">${esc(r.name)}</span><button data-x="${id}" title="Entfernen">✕</button></div>`).join("");
  el.querySelectorAll("[data-v]").forEach((cb) => (cb.onchange = () => { const r = refs.get(+cb.dataset.v); r.visible = cb.checked; r.meshes.forEach((m) => (m.visible = cb.checked)); }));
  el.querySelectorAll("[data-x]").forEach((b) => (b.onclick = () => { const id = +b.dataset.x; refs.get(id).meshes.forEach(clearObj); refs.delete(id); invoke("remove_reference", { id }); renderRefs(); }));
}
$("#refTransp").onchange = () => {
  const t = $("#refTransp").checked;
  for (const r of refs.values()) for (const m of r.meshes) {
    const glass = m.userData.cls === 2;
    m.material.transparent = t || glass; m.material.opacity = t || glass ? 0.32 : 1; m.material.depthWrite = !(t || glass); m.material.needsUpdate = true;
  }
};
async function showRefDetails(id, idx) {
  try {
    const e = await invoke("ref_element", { id, index: idx });
    const r = refs.get(id);
    $("#props").innerHTML = `<h3>Referenzmodell</h3><div class="big">${esc(e.class)}</div><table>
      <tr><td>Modell</td><td>${esc(r?.name)}</td></tr><tr><td>Name</td><td>${esc(e.name || "–")}</td></tr><tr><td>GlobalId</td><td>${esc(e.guid)}</td></tr></table>
      <p class="hint">Referenzmodelle dienen nur der Ansicht und fließen nicht in den Mengenauszug ein.</p>`;
  } catch (err) { toast(String(err), true); }
}

// Acertos em malhas sólidas (modelo principal „Alto“ e referências)
const ray = new THREE.Raycaster();
function meshHit(e) {
  const objs = [...mainMeshes, ...[...refs.values()].flatMap((r) => r.meshes)].filter((m) => m.visible);
  if (!objs.length) return null;
  const rect = renderer.domElement.getBoundingClientRect();
  ray.setFromCamera(new THREE.Vector2(((e.clientX - rect.left) / rect.width) * 2 - 1, -((e.clientY - rect.top) / rect.height) * 2 + 1), camera);
  const hits = ray.intersectObjects(objs, false).filter((h) => !clipping.length || clipping.every((pl) => pl.distanceToPoint(h.point) >= 0));
  // preferir o modelo principal à referência
  const h = hits.find((x) => !x.object.userData.ref && (x.distance - hits[0].distance) < 0.5) || hits[0];
  if (!h) return null;
  return { obj: h.object, elem: elemOfTri(h.object.userData.ranges, h.faceIndex), point: h.point };
}

// Ponto global para medição (malhas, senão eixo do tubo/ponto próximo na tela)
function pickWorld(e) {
  const mh = meshHit(e);
  if (mh) return mh.point.clone();
  const rect = renderer.domElement.getBoundingClientRect();
  const mx = e.clientX - rect.left, my = e.clientY - rect.top, W = rect.width, H = rect.height;
  const g = st.data.geometry, v = new THREE.Vector3();
  const proj = (x, y, z) => { v.set(x, y, z).project(camera); return v.z > 1 || v.z < -1 ? null : [(v.x + 1) / 2 * W, (1 - v.y) / 2 * H]; };
  let best = null, bestD = 12;
  for (let i = 0; i < g.point_elem.length; i++) {
    const p = proj(g.points[i * 3], g.points[i * 3 + 1], g.points[i * 3 + 2]); if (!p) continue;
    const d = Math.hypot(p[0] - mx, p[1] - my);
    if (d < bestD) { bestD = d; best = new THREE.Vector3(g.points[i * 3], g.points[i * 3 + 1], g.points[i * 3 + 2]); }
  }
  for (let s2 = 0; s2 < g.line_elem.length; s2++) {
    const o = s2 * 6, A = new THREE.Vector3(g.lines[o], g.lines[o + 1], g.lines[o + 2]), B = new THREE.Vector3(g.lines[o + 3], g.lines[o + 4], g.lines[o + 5]);
    const a = proj(A.x, A.y, A.z), b = proj(B.x, B.y, B.z); if (!a || !b) continue;
    const dx = b[0] - a[0], dy = b[1] - a[1], L2 = dx * dx + dy * dy;
    const t = L2 ? Math.max(0, Math.min(1, ((mx - a[0]) * dx + (my - a[1]) * dy) / L2)) : 0;
    const d = Math.hypot(a[0] + t * dx - mx, a[1] + t * dy - my);
    if (d < bestD) { bestD = d; best = A.clone().lerp(B, t); }
  }
  return best;
}

// Medição
function setMeasure(on) {
  measureMode = on; measureStart = null;
  $("#tMeasure").classList.toggle("active", on);
  $("#measurePanel").hidden = !on;
  $("#viewport").classList.toggle("measuring", on);
  $("#measHint").textContent = "Ersten Punkt anklicken …";
  if (on) { $("#sectionPanel").hidden = true; $("#tSection").classList.toggle("active", clipping.length > 0); }
}
$("#tMeasure").onclick = () => setMeasure(!measureMode);
$("#measOff").onclick = () => setMeasure(false);
$("#measClear").onclick = () => clearMeasures();
function clearMeasures() {
  for (const m of measures) { clearObj(m.obj); m.label.remove(); }
  measures.length = 0; measureStart = null;
  if ($("#measList")) $("#measList").innerHTML = "";
}
function measureClick(e) {
  const p = pickWorld(e);
  if (!p) { toast("Kein Bauteil getroffen – bitte direkt auf ein Bauteil klicken.", true, 2500); return; }
  if (!measureStart) { measureStart = p; $("#measHint").textContent = "Zweiten Punkt anklicken …"; return; }
  const a = measureStart, b = p; measureStart = null;
  const geo = new THREE.BufferGeometry().setFromPoints([a, b]);
  const col = document.documentElement.dataset.theme === "dark" ? "#ffffff" : "#0b2545";
  const obj = new THREE.Line(geo, new THREE.LineBasicMaterial({ color: col, depthTest: false }));
  obj.renderOrder = 5; scene.add(obj);
  const label = document.createElement("div"); label.className = "mlabel";
  const d = a.distanceTo(b);
  label.textContent = fmtN(d, 3) + " m";
  $("#labels").appendChild(label);
  measures.push({ a, b, obj, label });
  $("#measHint").textContent = "Ersten Punkt anklicken …";
  $("#measList").innerHTML = measures.map((m, i) => {
    const dd = m.a.distanceTo(m.b);
    return `<div class="mrow"><span>#${i + 1} <b>${fmtN(dd, 3)} m</b></span><span class="d">ΔX ${fmtN(Math.abs(m.b.x - m.a.x), 2)} · ΔY ${fmtN(Math.abs(m.b.y - m.a.y), 2)} · ΔZ ${fmtN(Math.abs(m.b.z - m.a.z), 2)}</span></div>`;
  }).join("");
}
function updateLabels() {
  if (!measures.length) return;
  const rect = renderer.domElement.getBoundingClientRect(), v = new THREE.Vector3();
  for (const m of measures) {
    v.copy(m.a).add(m.b).multiplyScalar(0.5).project(camera);
    const vis = v.z < 1 && v.z > -1;
    m.label.style.display = vis ? "" : "none";
    m.label.style.left = ((v.x + 1) / 2 * rect.width) + "px";
    m.label.style.top = ((1 - v.y) / 2 * rect.height) + "px";
  }
}

// Corte
let secAxis = "z", secFlip = false;
function sceneBox() {
  const box = new THREE.Box3();
  [pipeObj, fitObj, sprObj, accObj, ...mainMeshes, ...[...refs.values()].flatMap((r) => r.meshes)].forEach((o) => { if (o) box.expandByObject(o); });
  return box;
}
function setSection(axis) {
  const was = clipping.length;
  if (!axis) {
    clipping.length = 0;
    $("#tSection").classList.remove("active");
  } else {
    secAxis = axis;
    if (!clipping.length) clipping.push(clipPlane);
    $("#tSection").classList.add("active");
    $$("#secAxis button").forEach((b) => b.classList.toggle("active", b.dataset.ax === axis));
    updateSection();
  }
  if (was !== clipping.length && scene) scene.traverse((o) => { if (o.material) { o.material.clippingPlanes = clipping; o.material.needsUpdate = true; } });
}
function updateSection() {
  if (!clipping.length) return;
  const box = sceneBox(); if (box.isEmpty()) return;
  const k = { x: 0, y: 1, z: 2 }[secAxis];
  const lo = box.min.getComponent(k), hi = box.max.getComponent(k);
  const val = lo + (hi - lo) * (+$("#secPos").value / 1000);
  const n = new THREE.Vector3(); n.setComponent(k, secFlip ? 1 : -1);
  clipPlane.set(n, secFlip ? -val : val);
  $("#secVal").textContent = `${secAxis.toUpperCase()} = ${fmtN(val - lo, 2)} m (von ${fmtN(hi - lo, 2)} m)`;
}
$("#tSection").onclick = () => {
  const open = $("#sectionPanel").hidden;
  $("#sectionPanel").hidden = !open;
  if (open) { setMeasure(false); $("#sectionPanel").hidden = false; if (!clipping.length) setSection(secAxis); }
};
$$("#secAxis button").forEach((b) => (b.onclick = () => setSection(b.dataset.ax)));
$("#secPos").oninput = updateSection;
$("#secFlip").onclick = () => { secFlip = !secFlip; updateSection(); };
$("#secOff").onclick = () => { setSection(null); $("#sectionPanel").hidden = true; };

// ------------------------------------------------------------ Sistemas
const ANL_LABEL = { Alle: "Alle Anlagen", Sprinkler: "Sprinkler", Hydranten: "Hydranten", Trockensteigleitung: "Trockensteigleitung", "Nicht zugeordnet": "Nicht zugeordnet" };
const EXPORT_LABEL = { Alle: "Gesamt-Mengenauszug exportieren", Sprinkler: "Sprinkler-Mengenauszug exportieren", Hydranten: "Hydranten-Mengenauszug exportieren", Trockensteigleitung: "Trockensteigleitung-Mengenauszug exportieren", "Nicht zugeordnet": "Nicht zugeordnete exportieren" };
function renderAnlagen() {
  const list = st.data.anlagen.filter((a) => a.count > 0);
  const total = list.reduce((x, a) => x + a.count, 0);
  const items = list.length > 1 ? [{ name: "Alle", count: total }, ...list] : list;
  $("#anlagen").innerHTML = items.map((a) => `<button class="anl ${a.name === st.anlage ? "active" : ""}" data-a="${esc(a.name)}">${esc(ANL_LABEL[a.name] || a.name)}<span class="ct">${fmtN(a.count)}</span></button>`).join("");
  $$("#anlagen .anl").forEach((b) => (b.onclick = () => setAnlage(b.dataset.a)));
  $("#btnExport").textContent = EXPORT_LABEL[st.anlage] || "Nach Excel exportieren";
}
async function setAnlage(a) {
  st.anlage = a;
  try {
    st.data.takeoff = await invoke("takeoff", { anlage: a });
  } catch (e) { toast(String(e), true); return; }
  renderAnlagen();
  renderQto();
  highlight([]);
}

// ------------------------------------------------------------ Levantamento de quantitativos
function renderQto() {
  const t = st.data.takeoff;
  const ql = ["Vollständig", "Lücken im Modell", "Wichtige Daten fehlen"][t.quality];
  const nNotes = t.notes.reduce((a, n) => a + 1, 0);
  $("#kpis").innerHTML = `
    <div class="card kpi"><div class="v">${fmtN(t.total_pipe_m, 1)} m</div><div class="l">Rohr (${fmtN(t.pipe_count)} Segmente) · ${t.insulation_in_model ? `davon gedämmt ${fmtN(t.insulated_m, 1)} m` : "keine Dämmung im Modell"}</div></div>
    <div class="card kpi"><div class="v">${fmtN(t.fitting_count)}</div><div class="l">Formstücke</div></div>
    ${kpiDevices(t)}
    <div class="card kpi q"><div class="v"><div class="mini" id="timmyQ"></div><span class="amp g${t.quality}"></span>${ql}</div><div class="l">${nNotes} Hinweis${nNotes === 1 ? "" : "e"} – siehe Reiter „Hinweise“</div></div>`;
  mountTimmy($("#timmyQ"), ["happy", "idle", "sad"][t.quality]);
  if (t.quality === 0) setTimmy($("#timmyQ"), "happy", "idle", 2000);
  $$("#qtoTabs .sub").forEach((b) => (b.textContent = b.dataset.q === "notes" ? `Hinweise (${nNotes})` : b.textContent.replace(/ \(\d+\)$/, "")));
  renderTable();
}
function kpiDevices(t) {
  let v, l;
  if (t.anlage === "Hydranten") { v = t.hydrant_count; l = "Hydranten"; }
  else if (t.anlage === "Trockensteigleitung") { v = t.outlet_count; l = `Entnahmestellen · ${fmtN(t.feed_count)} Einspeisung${t.feed_count === 1 ? "" : "en"}`; }
  else if (t.anlage === "Sprinkler") { v = t.sprinkler_count; l = "Sprinkler"; }
  else { v = t.sprinkler_count; l = `Sprinkler · ${fmtN(t.hydrant_count)} Hydranten · ${fmtN(t.outlet_count)} Entnahmestellen`; }
  return `<div class="card kpi"><div class="v">${fmtN(v)}</div><div class="l">${l} · ${fmtN(t.accessory_count)} Armaturen & Zubehör gesamt</div></div>`;
}
function renderTable() {
  const t = st.data.takeoff, q = st.qtoTab, tb = $("#qtoTable");
  let head, rows, foot = "";
  if (q === "pipes") {
    head = '<th>Nennweite</th><th>Material</th><th class="n">Anzahl</th><th class="n">Länge [m]</th>';
    rows = t.pipes.map((r) => `<td><i class="sw" style="display:inline-block;width:18px;height:5px;border-radius:3px;background:${dnColor(r.dn)};margin-right:8px;vertical-align:middle"></i>${esc(r.dn)}</td><td>${esc(r.material)}</td><td class="n">${fmtN(r.count)}</td><td class="n">${fmtN(r.length_m, 2)}</td>`);
    foot = `<td>Summe</td><td></td><td class="n">${fmtN(t.pipe_count)}</td><td class="n">${fmtN(t.total_pipe_m, 2)}</td>`;
  } else if (q === "insulation") {
    head = '<th>Dämmung</th><th>Stärke</th><th>Nennweite</th><th class="n">Anzahl</th><th class="n">Länge [m]</th>';
    rows = t.insulation.map((r) => `<td>${r.kind === "ohne Dämmung" ? '<span class="muted">ohne Dämmung</span>' : esc(r.kind)}</td><td>${esc(r.material)}</td><td>${esc(r.dn)}</td><td class="n">${fmtN(r.count)}</td><td class="n">${fmtN(r.length_m, 2)}</td>`);
    if (!t.insulation_in_model) rows = ['<td colspan="5" class="muted">Im Modell ist keine Dämmung hinterlegt (weder Dämm-Bauteile noch Dämm-Eigenschaften an den Rohren).</td>'];
    foot = t.insulation_in_model ? `<td>Gedämmt</td><td></td><td></td><td></td><td class="n">${fmtN(t.insulated_m, 2)}</td>` : "";
  } else if (q === "fittings") {
    head = '<th>Art</th><th>Nennweite</th><th>Material</th><th class="n">Stück</th>';
    rows = t.fittings.map((r) => `<td>${esc(r.kind)}</td><td>${esc(r.dn)}</td><td>${esc(r.material)}</td><td class="n">${fmtN(r.count)}</td>`);
    foot = `<td>Summe</td><td></td><td></td><td class="n">${fmtN(t.fitting_count)}</td>`;
  } else if (q === "accessories") {
    head = '<th>Art</th><th>Nennweite</th><th>Typ</th><th class="n">Stück</th>';
    rows = t.accessories.map((r) => `<td>${esc(r.kind)}</td><td>${esc(r.dn)}</td><td>${esc(r.type_name)}</td><td class="n">${fmtN(r.count)}</td>`);
    foot = `<td>Summe</td><td></td><td></td><td class="n">${fmtN(t.accessory_count)}</td>`;
  } else {
    head = '<th>Hinweis</th><th class="n">Anzahl Bauteile</th>';
    rows = t.notes.map((n) => `<td>${esc(n.kind)}</td><td class="n">${fmtN(n.count)}</td>`);
    if (!rows.length) rows = ['<td colspan="2" class="muted">Keine Hinweise – das Modell ist vollständig.</td>'];
  }
  const src = q === "notes" ? t.notes : q === "insulation" && !t.insulation_in_model ? [] : t[q];
  tb.innerHTML = `<thead><tr>${head}</tr></thead><tbody>${rows.map((r, i) => `<tr data-i="${i}">${r}</tr>`).join("")}</tbody>${foot ? `<tfoot><tr>${foot}</tr></tfoot>` : ""}`;
  tb.querySelectorAll("tbody tr[data-i]").forEach((tr) => (tr.onclick = () => {
    const row = src?.[+tr.dataset.i]; if (!row) return;
    tb.querySelectorAll("tr.sel").forEach((x) => x.classList.remove("sel"));
    tr.classList.add("sel");
    highlight(row.elements);
    toast(`${fmtN(row.elements.length)} Bauteile in der 3D-Ansicht markiert`);
  }));
}
$$("#qtoTabs .sub").forEach((b) => (b.onclick = () => {
  st.qtoTab = b.dataset.q;
  $$("#qtoTabs .sub").forEach((x) => x.classList.toggle("active", x === b));
  renderTable();
}));

async function doExport() {
  if (!st.data) return;
  const suffix = { Alle: "Mengenauszug_gesamt", Sprinkler: "Mengenauszug_Sprinkler", Hydranten: "Mengenauszug_Hydranten", Trockensteigleitung: "Mengenauszug_Trockensteigleitung" }[st.anlage] || "Mengenauszug";
  const def = baseName(st.path).replace(/\.(ifc|ifczip)$/i, "") + "_" + suffix + ".xlsx";
  const p = await T.dialog.save({ defaultPath: def, filters: [{ name: "Excel", extensions: ["xlsx"] }] });
  if (!p) return;
  try {
    const out = await invoke("export_excel", { path: p, anlage: st.anlage });
    toast("Gespeichert: " + out, false, 5000);
  } catch (e) { toast(String(e), true, 8000); }
}
$("#btnExport").onclick = doExport;
$("#btnExportTop").onclick = doExport;

// Início com arquivo (duplo clique em .ifc)
invoke("startup_file").then((p) => { if (p) openFile(p); }).catch((e) => toast("Start: " + e, true, 10000));
