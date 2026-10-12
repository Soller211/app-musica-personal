"use strict";

// ============================================================================
// Puente con Rust (Tauri). Si se abre el HTML en un navegador normal, se usa
// un backend vacío para poder ver la interfaz sin la app.
// ============================================================================
const TAURI = window.__TAURI__;
const invoke = TAURI ? TAURI.core.invoke : async (cmd) => {
  if (cmd.startsWith("get_")) return [];
  if (cmd === "is_mobile") return matchMedia("(max-width: 760px)").matches;
  throw new Error("Abre la app con Tauri para usar esta función");
};
const fileSrc = (path) => (path && TAURI ? TAURI.core.convertFileSrc(path) : "");

// ============================================================================
// Utilidades
// ============================================================================
const $ = (sel, root = document) => root.querySelector(sel);
const esc = (s) =>
  String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const fmt = (secs) => {
  if (!isFinite(secs) || secs < 0) secs = 0;
  secs = Math.floor(secs);
  const h = Math.floor(secs / 3600), m = Math.floor((secs % 3600) / 60), s = secs % 60;
  return h ? `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}` : `${m}:${String(s).padStart(2, "0")}`;
};
const fmtLong = (secs) => {
  const m = Math.round(secs / 60);
  return m >= 60 ? `${Math.floor(m / 60)} h ${m % 60} min` : `${m} min`;
};
const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;
// GIF transparente: evita el borde que dibuja el navegador en <img> sin src.
const BLANK = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";
const coverImg = (path, cls = "cover") =>
  `<img class="${cls}" src="${path ? esc(fileSrc(path)) : BLANK}" alt="" loading="lazy" />`;
const norm = (s) => String(s || "").normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase();
const albumKey = (t) => `${t.albumArtist}\u0001${t.album}`;
const store = {
  get(k, d) { try { const v = localStorage.getItem(k); return v === null ? d : JSON.parse(v); } catch { return d; } },
  set(k, v) { try { localStorage.setItem(k, JSON.stringify(v)); } catch {} },
};

let toastTimer;
function toast(msg, ms = 2500) {
  const el = $("#toast");
  el.textContent = msg;
  el.hidden = false;
  clearTimeout(toastTimer);
  if (ms) toastTimer = setTimeout(() => (el.hidden = true), ms);
}

// ============================================================================
// Estado de la biblioteca
// ============================================================================
const lib = {
  tracks: [],
  byId: new Map(),
  playlists: [],
  albums: new Map(), // key -> {key, name, artist, cover, year, tracks[]}
  artists: new Map(), // nombre -> {name, cover, tracks[], albums Set}
  folders: [],
  isMobile: false,
};

function indexLibrary() {
  lib.byId = new Map(lib.tracks.map((t) => [t.id, t]));
  lib.albums = new Map();
  lib.artists = new Map();
  for (const t of lib.tracks) {
    const k = albumKey(t);
    let a = lib.albums.get(k);
    if (!a) lib.albums.set(k, (a = { key: k, name: t.album, artist: t.albumArtist, cover: null, year: null, tracks: [], addedAt: 0 }));
    a.tracks.push(t);
    a.cover ||= t.cover;
    a.year ||= t.year;
    a.addedAt = Math.max(a.addedAt, t.addedAt);

    for (const name of new Set([t.artist, t.albumArtist])) {
      let ar = lib.artists.get(name);
      if (!ar) lib.artists.set(name, (ar = { name, cover: null, tracks: [], albums: new Set() }));
      if (name === t.artist || !ar.tracks.includes(t)) ar.tracks.push(t);
      ar.albums.add(k);
      ar.cover ||= t.cover;
    }
  }
}

async function loadLibrary() {
  const [tracks, playlists] = await Promise.all([invoke("get_tracks"), invoke("get_playlists")]);
  lib.tracks = tracks;
  lib.playlists = playlists;
  indexLibrary();
  renderSidePlaylists();
  player.syncQueueWithLibrary();
  render();
}

async function reloadPlaylists() {
  lib.playlists = await invoke("get_playlists");
  renderSidePlaylists();
}

// ============================================================================
// Reproductor
// ============================================================================
const audio = $("#audio");

const player = {
  queue: [], // ids en orden original
  order: [], // índices de reproducción (aleatorio o no)
  pos: -1, // posición dentro de `order`
  shuffle: store.get("shuffle", false),
  repeat: store.get("repeat", "off"), // off | all | one
  source: "",
  counted: false,

  get current() {
    const i = this.order[this.pos];
    return i === undefined ? null : lib.byId.get(this.queue[i]) || null;
  },

  play(ids, startId = null, source = "") {
    ids = ids.filter((id) => lib.byId.has(id));
    if (!ids.length) return;
    this.queue = ids;
    this.source = source;
    let start = startId == null ? 0 : Math.max(0, ids.indexOf(startId));
    if (startId == null && this.shuffle) start = Math.floor(Math.random() * ids.length);
    this.buildOrder(start);
    this.pos = this.order.indexOf(start);
    this.load(true);
  },

  buildOrder(firstIndex) {
    const idx = this.queue.map((_, i) => i);
    if (this.shuffle) {
      const rest = idx.filter((i) => i !== firstIndex);
      for (let i = rest.length - 1; i > 0; i--) {
        const j = Math.floor(Math.random() * (i + 1));
        [rest[i], rest[j]] = [rest[j], rest[i]];
      }
      this.order = [firstIndex, ...rest];
    } else {
      this.order = idx;
    }
  },

  load(autoplay, startAt = 0) {
    const t = this.current;
    if (!t) return;
    this.counted = false;
    audio.src = fileSrc(t.path);
    if (startAt) audio.addEventListener("loadedmetadata", () => (audio.currentTime = startAt), { once: true });
    if (autoplay) audio.play().catch((e) => toast("No se pudo reproducir: " + e.message));
    updateNowPlaying();
    this.persist();
  },

  toggle() {
    if (!this.current) {
      if (lib.tracks.length) this.play(lib.tracks.map((t) => t.id), null, "Tu biblioteca");
      return;
    }
    if (audio.paused) audio.play().catch(() => {});
    else audio.pause();
  },

  next(auto = false) {
    if (!this.current) return;
    if (auto && this.repeat === "one") {
      audio.currentTime = 0;
      audio.play();
      return;
    }
    if (this.pos < this.order.length - 1) this.pos++;
    else if (this.repeat === "all" || !auto) this.pos = 0;
    else {
      audio.pause();
      audio.currentTime = 0;
      return;
    }
    this.load(true);
  },

  prev() {
    if (!this.current) return;
    if (audio.currentTime > 3) {
      audio.currentTime = 0;
      return;
    }
    this.pos = this.pos > 0 ? this.pos - 1 : this.order.length - 1;
    this.load(true);
  },

  setShuffle(on) {
    this.shuffle = on;
    store.set("shuffle", on);
    const cur = this.order[this.pos];
    if (cur !== undefined) {
      this.buildOrder(cur);
      this.pos = this.order.indexOf(cur);
    }
    updateModes();
    this.persist();
  },

  cycleRepeat() {
    this.repeat = { off: "all", all: "one", one: "off" }[this.repeat];
    store.set("repeat", this.repeat);
    updateModes();
  },

  /** Inserta canciones justo después de la actual. */
  playNext(ids) {
    if (!this.current) return this.play(ids);
    const insertAt = this.queue.length;
    this.queue.push(...ids);
    const newIdx = ids.map((_, i) => insertAt + i);
    this.order.splice(this.pos + 1, 0, ...newIdx);
    if (!this.shuffle) this.normalizeLinear();
    this.persist();
  },

  addToQueue(ids) {
    if (!this.current) return this.play(ids);
    const insertAt = this.queue.length;
    this.queue.push(...ids);
    this.order.push(...ids.map((_, i) => insertAt + i));
    this.persist();
  },

  /** Sin aleatorio, reconstruye `queue` siguiendo `order` para que sea lineal. */
  normalizeLinear() {
    this.queue = this.order.map((i) => this.queue[i]);
    this.order = this.queue.map((_, i) => i);
  },

  upcoming() {
    return this.order.slice(this.pos + 1).map((i) => this.queue[i]);
  },

  jumpTo(orderPos) {
    this.pos = orderPos;
    this.load(true);
  },

  syncQueueWithLibrary() {
    const cur = this.current;
    const curQi = this.order[this.pos];
    if (!this.queue.length) return;
    if (this.queue.every((id) => lib.byId.has(id))) {
      updateNowPlaying();
      return;
    }
    // Se borraron canciones: las quitamos de la cola.
    const keep = this.queue.map((id) => lib.byId.has(id));
    const remap = [];
    let n = 0;
    keep.forEach((k, i) => (remap[i] = k ? n++ : -1));
    this.queue = this.queue.filter((_, i) => keep[i]);
    this.order = this.order.filter((i) => keep[i]).map((i) => remap[i]);
    if (!cur || !lib.byId.has(cur.id)) {
      audio.pause();
      audio.removeAttribute("src");
      this.pos = Math.min(this.pos, this.order.length - 1);
      if (this.pos >= 0) this.load(false);
    } else {
      this.pos = this.order.indexOf(remap[curQi]);
    }
    updateNowPlaying();
    this.persist();
  },

  persist() {
    store.set("session", { queue: this.queue, order: this.order, pos: this.pos, source: this.source, time: audio.currentTime || 0 });
  },

  restore() {
    const s = store.get("session", null);
    if (!s || !Array.isArray(s.queue) || !s.queue.length) return;
    this.queue = s.queue;
    this.order = s.order;
    this.pos = s.pos;
    this.source = s.source || "";
    this.syncQueueWithLibrary();
    if (this.current) this.load(false, s.time || 0);
  },
};

// Eventos del elemento <audio>
audio.volume = store.get("volume", 1);
audio.addEventListener("play", updatePlayState);
audio.addEventListener("pause", updatePlayState);
audio.addEventListener("ended", () => player.next(true));
audio.addEventListener("loadedmetadata", updateTime);
audio.addEventListener("error", () => {
  const t = player.current;
  if (!t || !audio.src) return;
  toast(`No se puede reproducir «${t.title}» (formato no soportado o archivo movido)`, 4000);
});
let lastPersist = 0;
audio.addEventListener("timeupdate", () => {
  updateTime();
  const t = player.current;
  if (t && !player.counted && (audio.currentTime > 30 || audio.currentTime > audio.duration / 2)) {
    player.counted = true;
    t.playCount++;
    t.lastPlayed = Math.floor(Date.now() / 1000);
    invoke("record_play", { id: t.id }).catch(() => {});
  }
  if (Date.now() - lastPersist > 5000) {
    lastPersist = Date.now();
    player.persist();
  }
});

function setRange(el, frac) {
  el.value = Math.round(frac * Number(el.max));
  el.style.setProperty("--p", `${frac * 100}%`);
}

let seeking = false;
function updateTime() {
  const d = audio.duration || player.current?.duration || 0;
  const c = audio.currentTime || 0;
  const frac = d ? c / d : 0;
  $("#t-cur").textContent = $("#fp-cur").textContent = fmt(c);
  $("#t-dur").textContent = $("#fp-dur").textContent = fmt(d);
  $("#mini-progress").style.width = `${frac * 100}%`;
  if (!seeking) {
    setRange($("#seek"), frac);
    setRange($("#fp-seek"), frac);
  }
  if ("mediaSession" in navigator && d && navigator.mediaSession.setPositionState) {
    try { navigator.mediaSession.setPositionState({ duration: d, position: Math.min(c, d), playbackRate: 1 }); } catch {}
  }
}

function updatePlayState() {
  const icon = audio.paused ? ICONS.play : ICONS.pause;
  $("#btn-play").innerHTML = icon;
  $("#fp-play").innerHTML = icon;
  $("#btn-play").title = audio.paused ? "Reproducir" : "Pausa";
  if ("mediaSession" in navigator) navigator.mediaSession.playbackState = audio.paused ? "paused" : "playing";
}

function updateModes() {
  for (const id of ["#btn-shuffle", "#fp-shuffle"]) $(id).classList.toggle("on", player.shuffle);
  for (const id of ["#btn-repeat", "#fp-repeat"]) {
    $(id).classList.toggle("on", player.repeat !== "off");
    $(id).innerHTML = player.repeat === "one" ? ICONS.repeatOne : ICONS.repeat;
  }
  updateVolume();
}

function updateVolume() {
  setRange($("#volume"), audio.muted ? 0 : audio.volume);
  $("#btn-mute").innerHTML = audio.muted || audio.volume === 0 ? ICONS.mute : ICONS.volume;
}

function updateNowPlaying() {
  const t = player.current;
  $("#player").classList.toggle("empty", !t);
  $("#np-title").textContent = t ? t.title : "Nada sonando";
  $("#np-artist").textContent = t ? t.artist : "";
  $("#fp-title").textContent = t ? t.title : "";
  $("#fp-artist").textContent = t ? t.artist : "";
  $("#fp-from").textContent = player.source ? `Desde ${player.source}` : "Reproduciendo";
  const src = t && t.cover ? fileSrc(t.cover) : "";
  for (const id of ["#np-cover", "#fp-cover"]) {
    const img = $(id);
    img.src = src || BLANK;
    img.classList.add("cover");
  }
  $("#fp-bg").style.backgroundImage = src ? `url("${src}")` : "";
  updateFavButtons();
  document.querySelectorAll(".track.playing").forEach((el) => el.classList.remove("playing"));
  if (t) document.querySelectorAll(`.track[data-id="${t.id}"]`).forEach((el) => el.classList.add("playing"));

  if ("mediaSession" in navigator && t) {
    navigator.mediaSession.metadata = new MediaMetadata({
      title: t.title,
      artist: t.artist,
      album: t.album,
      artwork: src ? [{ src, sizes: "512x512" }] : [],
    });
  }
  document.title = t ? `${t.title} · ${t.artist}` : "Mi Música";
}

function updateFavButtons() {
  const t = player.current;
  for (const id of ["#np-fav", "#fp-fav"]) {
    const b = $(id);
    b.innerHTML = t && t.favorite ? ICONS.heartFill : ICONS.heart;
    b.classList.toggle("on", !!(t && t.favorite));
  }
}

if ("mediaSession" in navigator) {
  const ms = navigator.mediaSession;
  const set = (a, f) => { try { ms.setActionHandler(a, f); } catch {} };
  set("play", () => audio.play());
  set("pause", () => audio.pause());
  set("nexttrack", () => player.next());
  set("previoustrack", () => player.prev());
  set("seekto", (e) => (audio.currentTime = e.seekTime));
  set("seekbackward", () => (audio.currentTime = Math.max(0, audio.currentTime - 10)));
  set("seekforward", () => (audio.currentTime = Math.min(audio.duration || 0, audio.currentTime + 10)));
}

// ============================================================================
// Acciones sobre canciones
// ============================================================================
async function toggleFavorite(id) {
  const t = lib.byId.get(id);
  if (!t) return;
  const fav = !t.favorite;
  await invoke("set_favorite", { id, favorite: fav });
  t.favorite = fav;
  toast(fav ? "Añadida a Favoritas" : "Quitada de Favoritas", 1500);
  updateFavButtons();
  document.querySelectorAll(`.track[data-id="${id}"] .heart`).forEach((b) => {
    b.classList.toggle("on", fav);
    b.innerHTML = fav ? ICONS.heartFill : ICONS.heart;
  });
  if (route.name === "favorites") render();
}

async function deleteTrack(id) {
  const t = lib.byId.get(id);
  if (!t) return;
  const msg = t.imported
    ? `¿Eliminar «${t.title}»? Se borrará el archivo de la app.`
    : `¿Quitar «${t.title}» de la biblioteca? El archivo original no se borra, pero volverá a aparecer al re-escanear su carpeta.`;
  if (!(await confirmSheet(msg, "Eliminar"))) return;
  await invoke("delete_track", { id });
  await loadLibrary();
  toast("Canción eliminada");
}

async function importFiles(files) {
  files = [...files];
  if (!files.length) return;
  let ok = 0;
  const errors = [];
  for (const [i, f] of files.entries()) {
    toast(`Importando ${i + 1}/${files.length}: ${f.name}`, 0);
    try {
      const bytes = new Uint8Array(await f.arrayBuffer());
      await invoke("import_file", bytes, { headers: { "x-file-name": encodeURIComponent(f.name) } });
      ok++;
    } catch (e) {
      errors.push(`${f.name}: ${e}`);
    }
  }
  await loadLibrary();
  toast(errors.length ? `${ok} importadas, ${errors.length} con error` : `${plural(ok, "canción importada", "canciones importadas")}`, 4000);
  if (errors.length) console.warn(errors);
}

function scanSummary(r) {
  const parts = [];
  if (r.added) parts.push(`${r.added} nuevas`);
  if (r.updated) parts.push(`${r.updated} actualizadas`);
  if (r.removed) parts.push(`${r.removed} quitadas`);
  if (r.errors.length) parts.push(`${r.errors.length} con error`);
  return parts.length ? parts.join(", ") : "Sin cambios";
}

async function pickFolder() {
  try {
    toast("Escaneando…", 0);
    const r = await invoke("pick_folder");
    if (!r) return toast("Cancelado", 1000);
    await loadLibrary();
    toast(scanSummary(r), 4000);
  } catch (e) {
    toast(String(e), 4000);
  }
}

async function rescan() {
  toast("Re-escaneando carpetas…", 0);
  try {
    const r = await invoke("rescan");
    await loadLibrary();
    toast(scanSummary(r), 4000);
  } catch (e) {
    toast(String(e), 4000);
  }
}

// ============================================================================
// Hojas (menús contextuales y diálogos)
// ============================================================================
function openSheet(html, onClick) {
  const sheet = $("#sheet");
  sheet.innerHTML = html;
  sheet.hidden = false;
  $("#sheet-backdrop").hidden = false;
  sheet.onclick = (e) => {
    const b = e.target.closest("[data-act]");
    if (b && onClick) onClick(b.dataset.act, b);
  };
}
function closeSheet() {
  $("#sheet").hidden = true;
  $("#sheet-backdrop").hidden = true;
  $("#sheet").onclick = null;
}
$("#sheet-backdrop").onclick = closeSheet;

const opt = (act, icon, label, cls = "") =>
  `<button class="opt ${cls}" data-act="${act}">${ICONS[icon]}<span>${esc(label)}</span></button>`;

function confirmSheet(message, okLabel = "Aceptar") {
  return new Promise((resolve) => {
    openSheet(
      `<form><div>${esc(message)}</div><div class="row">
         <button type="button" class="btn ghost" data-act="no">Cancelar</button>
         <button type="button" class="btn" data-act="yes">${esc(okLabel)}</button></div></form>`,
      (act) => { closeSheet(); resolve(act === "yes"); }
    );
    $("#sheet-backdrop").onclick = () => { closeSheet(); resolve(false); $("#sheet-backdrop").onclick = closeSheet; };
  });
}

function promptSheet(title, value = "", okLabel = "Guardar") {
  return new Promise((resolve) => {
    openSheet(
      `<h3>${esc(title)}</h3><form id="prompt-form">
         <input id="prompt-input" value="${esc(value)}" autocomplete="off" />
         <div class="row"><button type="button" class="btn ghost" data-act="no">Cancelar</button>
         <button class="btn" type="submit">${esc(okLabel)}</button></div></form>`,
      (act) => { if (act === "no") { closeSheet(); resolve(null); } }
    );
    const input = $("#prompt-input");
    input.focus();
    input.select();
    $("#prompt-form").onsubmit = (e) => {
      e.preventDefault();
      closeSheet();
      resolve(input.value.trim());
    };
  });
}

function trackMenu(id, ctx = {}) {
  const t = lib.byId.get(id);
  if (!t) return;
  let html = `<div class="sheet-head">${coverImg(t.cover)}<div><div class="ellipsis"><b>${esc(t.title)}</b></div>
    <div class="muted ellipsis">${esc(t.artist)}</div></div></div>`;
  html += opt("fav", t.favorite ? "heartFill" : "heart", t.favorite ? "Quitar de Favoritas" : "Añadir a Favoritas");
  html += opt("next", "next2", "Reproducir a continuación");
  html += opt("queue", "queue", "Añadir a la cola");
  html += opt("playlist", "playlist", "Añadir a playlist");
  if (ctx.playlistId) html += opt("unlist", "trash", "Quitar de esta playlist");
  html += opt("album", "album", "Ir al álbum");
  html += opt("artist", "artist", "Ir al artista");
  html += opt("delete", "trash", t.imported ? "Eliminar canción" : "Quitar de la biblioteca", "danger");
  openSheet(html, async (act) => {
    closeSheet();
    if (act === "fav") toggleFavorite(id);
    if (act === "next") { player.playNext([id]); toast("Se reproducirá a continuación", 1500); }
    if (act === "queue") { player.addToQueue([id]); toast("Añadida a la cola", 1500); }
    if (act === "playlist") addToPlaylistMenu([id]);
    if (act === "unlist") removeFromPlaylist(ctx.playlistId, ctx.index);
    if (act === "album") go(`album/${encodeURIComponent(albumKey(t))}`);
    if (act === "artist") go(`artist/${encodeURIComponent(t.artist)}`);
    if (act === "delete") deleteTrack(id);
  });
}

function addToPlaylistMenu(ids) {
  let html = `<h3>Añadir a playlist</h3>` + opt("new", "plus", "Nueva playlist");
  for (const p of lib.playlists) html += opt(`p${p.id}`, "playlist", p.name);
  openSheet(html, async (act) => {
    closeSheet();
    let pid;
    if (act === "new") {
      const name = await promptSheet("Nueva playlist", "", "Crear");
      if (name === null) return;
      pid = (await invoke("create_playlist", { name })).id;
    } else pid = Number(act.slice(1));
    await invoke("add_to_playlist", { id: pid, trackIds: ids });
    await reloadPlaylists();
    const p = lib.playlists.find((x) => x.id === pid);
    toast(`Añadida a «${p ? p.name : "playlist"}»`, 1500);
    if (route.name === "playlist") render();
  });
}

async function removeFromPlaylist(pid, index) {
  const p = lib.playlists.find((x) => x.id === pid);
  if (!p) return;
  const ids = p.trackIds.filter((_, i) => i !== index);
  await invoke("set_playlist_tracks", { id: pid, trackIds: ids });
  await reloadPlaylists();
  render();
}

function playlistMenu(pid) {
  const p = lib.playlists.find((x) => x.id === pid);
  if (!p) return;
  openSheet(
    `<h3>${esc(p.name)}</h3>` + opt("rename", "edit", "Cambiar nombre") + opt("queue", "queue", "Añadir a la cola") +
      opt("delete", "trash", "Eliminar playlist", "danger"),
    async (act) => {
      closeSheet();
      if (act === "rename") {
        const name = await promptSheet("Cambiar nombre", p.name);
        if (!name) return;
        await invoke("rename_playlist", { id: pid, name });
      }
      if (act === "queue") return player.addToQueue(p.trackIds);
      if (act === "delete") {
        if (!(await confirmSheet(`¿Eliminar la playlist «${p.name}»? Las canciones no se borran.`, "Eliminar"))) return;
        await invoke("delete_playlist", { id: pid });
        go("playlists");
      }
      await reloadPlaylists();
      render();
    }
  );
}

async function newPlaylist() {
  const name = await promptSheet("Nueva playlist", "", "Crear");
  if (name === null) return;
  const p = await invoke("create_playlist", { name });
  await reloadPlaylists();
  go(`playlist/${p.id}`);
}

function showQueue() {
  const cur = player.current;
  if (!cur) return toast("La cola está vacía", 1500);
  let html = `<h3>Sonando ahora</h3>${queueRow(cur, player.pos, true)}`;
  const up = player.order.slice(player.pos + 1);
  html += `<h3 style="margin-top:12px">A continuación${player.source ? ` · ${esc(player.source)}` : ""}</h3>`;
  html += up.length ? up.slice(0, 200).map((qi, k) => queueRow(lib.byId.get(player.queue[qi]), player.pos + 1 + k)).join("") : `<div class="muted" style="padding:12px 18px">No hay más canciones</div>`;
  openSheet(html, (act) => {
    closeSheet();
    player.jumpTo(Number(act.slice(1)));
  });
}
const queueRow = (t, pos, current = false) =>
  !t ? "" : `<button class="opt queue-item ${current ? "current" : ""}" data-act="q${pos}">${coverImg(t.cover, "cover")}
     <span class="ellipsis"><span class="t">${esc(t.title)}</span><br><small class="muted">${esc(t.artist)}</small></span></button>`;

// ============================================================================
// Router (#/ruta) — así el botón "atrás" de Android funciona de forma natural
// ============================================================================
const route = { name: "home", arg: "" };
function go(path) {
  location.hash = `#/${path}`;
}
function parseRoute() {
  const h = location.hash.replace(/^#\/?/, "");
  const [name, ...rest] = h.split("/");
  route.name = name || "home";
  route.arg = decodeURIComponent(rest.join("/"));
}
window.addEventListener("hashchange", () => {
  parseRoute();
  render();
  $("#view").scrollTop = 0;
});

const NAV = [
  ["home", "home", "Inicio"],
  ["search", "search", "Buscar"],
  ["songs", "music", "Canciones"],
  ["albums", "album", "Álbumes"],
  ["artists", "artist", "Artistas"],
  ["favorites", "heart", "Favoritas"],
  ["playlists", "playlist", "Playlists"],
  ["settings", "settings", "Ajustes"],
];
const TABS = [
  ["home", "home", "Inicio"],
  ["search", "search", "Buscar"],
  ["library", "library", "Biblioteca"],
  ["settings", "settings", "Ajustes"],
];
const TAB_OF = { songs: "library", albums: "library", artists: "library", favorites: "library", playlists: "library", album: "library", artist: "library", playlist: "library" };

function renderNav() {
  $("#nav").innerHTML = NAV.map(([r, i, l]) => `<a href="#/${r}" data-r="${r}">${ICONS[i]}<span>${l}</span></a>`).join("");
  $("#tabbar").innerHTML = TABS.map(([r, i, l]) => `<a href="#/${r}" data-r="${r}">${ICONS[i]}<span>${l}</span></a>`).join("");
}
function highlightNav() {
  document.querySelectorAll("#nav a").forEach((a) => a.classList.toggle("active", a.dataset.r === route.name));
  const tab = TAB_OF[route.name] || route.name;
  document.querySelectorAll("#tabbar a").forEach((a) => a.classList.toggle("active", a.dataset.r === tab));
  document.querySelectorAll(".side-item").forEach((a) => a.classList.toggle("active", route.name === "playlist" && a.dataset.id === route.arg));
}

function renderSidePlaylists() {
  $("#side-playlists").innerHTML = lib.playlists
    .map((p) => `<a class="side-item" href="#/playlist/${p.id}" data-id="${p.id}"><span>${esc(p.name)}</span></a>`)
    .join("");
  highlightNav();
}

// ============================================================================
// Vistas
// ============================================================================
function trackRows(tracks, opts = {}) {
  const cur = player.current;
  return `<div class="tracks ${opts.noAlbum ? "no-album" : ""}" data-list>${tracks
    .map((t, i) => `<div class="track ${cur && cur.id === t.id ? "playing" : ""}" data-id="${t.id}" data-i="${i}">
      <div class="n">${opts.numbers === "track" ? t.trackNo ?? "–" : i + 1}</div>
      <div class="main">${opts.noCover ? "" : coverImg(t.cover)}<div style="min-width:0">
        <div class="t ellipsis">${esc(t.title)}</div><div class="a ellipsis">${esc(t.artist)}</div></div></div>
      <div class="al ellipsis">${esc(t.album)}</div>
      <button class="icon-btn heart ${t.favorite ? "on" : ""}" data-fav>${t.favorite ? ICONS.heartFill : ICONS.heart}</button>
      <div class="d">${fmt(t.duration)}</div>
      <button class="icon-btn" data-more>${ICONS.more}</button>
    </div>`)
    .join("")}</div>`;
}

/** Conecta los clics de una lista de canciones. */
function bindTrackList(container, tracks, source, ctx = {}) {
  container.querySelectorAll("[data-list]").forEach((list) => {
    list.addEventListener("click", (e) => {
      const row = e.target.closest(".track");
      if (!row) return;
      const id = Number(row.dataset.id);
      if (e.target.closest("[data-fav]")) return toggleFavorite(id);
      if (e.target.closest("[data-more]")) return trackMenu(id, { ...ctx, index: Number(row.dataset.i) });
      player.play(tracks.map((t) => t.id), id, source);
    });
    list.addEventListener("contextmenu", (e) => {
      const row = e.target.closest(".track");
      if (!row) return;
      e.preventDefault();
      trackMenu(Number(row.dataset.id), { ...ctx, index: Number(row.dataset.i) });
    });
  });
}

function albumCards(albums) {
  return albums
    .map((a) => `<div class="card" data-album="${esc(a.key)}">${coverImg(a.cover)}
      <div class="t ellipsis">${esc(a.name)}</div><div class="s ellipsis">${a.year ? a.year + " · " : ""}${esc(a.artist)}</div>
      <button class="play-btn green" data-play>${ICONS.play}</button></div>`)
    .join("");
}
function artistCards(artists) {
  return artists
    .map((a) => `<div class="card" data-artist="${esc(a.name)}">${coverImg(a.cover, "cover round")}
      <div class="t ellipsis">${esc(a.name)}</div><div class="s">Artista</div>
      <button class="play-btn green" data-play>${ICONS.play}</button></div>`)
    .join("");
}
function playlistCards(lists) {
  return lists
    .map((p) => {
      const first = p.trackIds.map((id) => lib.byId.get(id)).find((t) => t && t.cover);
      return `<div class="card" data-playlist="${p.id}">${coverImg(first && first.cover)}
        <div class="t ellipsis">${esc(p.name)}</div><div class="s">${plural(p.trackIds.length, "canción", "canciones")}</div>
        <button class="play-btn green" data-play>${ICONS.play}</button></div>`;
    })
    .join("");
}
function bindCards(container) {
  container.addEventListener("click", (e) => {
    const card = e.target.closest(".card");
    if (!card) return;
    const play = !!e.target.closest("[data-play]");
    if (card.dataset.album !== undefined) {
      const a = lib.albums.get(card.dataset.album);
      if (play) return a && player.play(a.tracks.map((t) => t.id), null, a.name);
      return go(`album/${encodeURIComponent(card.dataset.album)}`);
    }
    if (card.dataset.artist !== undefined) {
      const a = lib.artists.get(card.dataset.artist);
      if (play) return a && player.play(a.tracks.map((t) => t.id), null, a.name);
      return go(`artist/${encodeURIComponent(card.dataset.artist)}`);
    }
    if (card.dataset.playlist) {
      const p = lib.playlists.find((x) => x.id === Number(card.dataset.playlist));
      if (play) return p && player.play(p.trackIds, null, p.name);
      return go(`playlist/${card.dataset.playlist}`);
    }
  });
}

function emptyLibrary() {
  return `<div class="empty"><h2>Tu biblioteca está vacía</h2>
    <p>${lib.isMobile ? "Importa canciones desde tu teléfono: se guardan dentro de la app y podrás escucharlas sin conexión." : "Añade una carpeta con tu música o importa canciones sueltas."}</p>
    ${lib.isMobile ? "" : `<button class="btn" data-act="folder">${ICONS.folder}Añadir carpeta</button> `}
    <button class="btn ${lib.isMobile ? "" : "ghost"}" data-act="import">${ICONS.upload}Importar canciones</button></div>`;
}

function playAllActions(source, extra = "") {
  return `<div class="actions"><button class="play-btn green" data-act="play" title="Reproducir">${ICONS.play}</button>
    <button class="icon-btn ${player.shuffle ? "on" : ""}" data-act="shuffle" title="Aleatorio">${ICONS.shuffle}</button>${extra}</div>`;
}

const VIEWS = {
  home(v) {
    const h = new Date().getHours();
    const greet = h < 12 ? "Buenos días" : h < 20 ? "Buenas tardes" : "Buenas noches";
    if (!lib.tracks.length) return `<h1 class="page">${greet}</h1>${emptyLibrary()}`;

    const recentAlbums = [...lib.albums.values()].sort((a, b) => b.addedAt - a.addedAt).slice(0, 12);
    const recentPlayed = lib.tracks.filter((t) => t.lastPlayed).sort((a, b) => b.lastPlayed - a.lastPlayed).slice(0, 10);
    const top = lib.tracks.filter((t) => t.playCount).sort((a, b) => b.playCount - a.playCount).slice(0, 10);
    const favCount = lib.tracks.filter((t) => t.favorite).length;

    let html = `<h1 class="page">${greet}</h1><div class="quick">
      <div class="q fav" data-go="favorites"><div class="cover">${ICONS.heartFill}</div>Favoritas · ${favCount}</div>
      <div class="q" data-go="songs"><div class="cover"></div>Todas las canciones</div>
      ${lib.playlists.slice(0, 4).map((p) => `<div class="q" data-go="playlist/${p.id}"><div class="cover"></div><span class="ellipsis">${esc(p.name)}</span></div>`).join("")}
    </div>`;
    if (recentPlayed.length) html += `<h2>Escuchado recientemente</h2><div id="h-recent">${trackRows(recentPlayed)}</div>`;
    html += `<h2>Añadido recientemente</h2><div class="row-scroll">${albumCards(recentAlbums)}</div>`;
    if (top.length) html += `<h2>Lo que más escuchas</h2><div id="h-top">${trackRows(top)}</div>`;
    v.innerHTML = html;
    v.querySelectorAll("[data-go]").forEach((q) => (q.onclick = () => go(q.dataset.go)));
    if (recentPlayed.length) bindTrackList($("#h-recent", v), recentPlayed, "Escuchado recientemente");
    if (top.length) bindTrackList($("#h-top", v), top, "Lo más escuchado");
  },

  search(v) {
    v.innerHTML = `<h1 class="page">Buscar</h1><div class="search-box">${ICONS.search}
      <input id="q" type="search" placeholder="¿Qué quieres escuchar?" value="${esc(VIEWS.search.q || "")}" autocomplete="off" /></div>
      <div id="results"></div>`;
    const input = $("#q", v);
    const run = () => {
      const q = norm((VIEWS.search.q = input.value).trim());
      const res = $("#results", v);
      if (!q) {
        res.innerHTML = lib.tracks.length ? `<h2>Explorar</h2><div class="grid">${albumCards([...lib.albums.values()].slice(0, 24))}</div>` : emptyLibrary();
        return;
      }
      const songs = lib.tracks.filter((t) => norm(`${t.title} ${t.artist} ${t.album}`).includes(q));
      const albums = [...lib.albums.values()].filter((a) => norm(`${a.name} ${a.artist}`).includes(q));
      const artists = [...lib.artists.values()].filter((a) => norm(a.name).includes(q));
      let html = "";
      if (songs.length) html += `<h2>Canciones</h2>${trackRows(songs.slice(0, 50))}`;
      if (artists.length) html += `<h2>Artistas</h2><div class="row-scroll">${artistCards(artists.slice(0, 20))}</div>`;
      if (albums.length) html += `<h2>Álbumes</h2><div class="row-scroll">${albumCards(albums.slice(0, 20))}</div>`;
      res.innerHTML = html || `<div class="empty"><h2>Sin resultados para «${esc(input.value)}»</h2></div>`;
      bindTrackList(res, songs.slice(0, 50), `Búsqueda «${input.value}»`);
    };
    input.oninput = run;
    run();
    if (!lib.isMobile) input.focus();
  },

  library(v) {
    // Vista de biblioteca en móvil: accesos a cada sección.
    v.innerHTML = `<h1 class="page">Tu biblioteca</h1>
      <div class="chips">${[["playlists", "Playlists"], ["albums", "Álbumes"], ["artists", "Artistas"], ["songs", "Canciones"], ["favorites", "Favoritas"]]
        .map(([r, l]) => `<a class="chip" href="#/${r}" style="text-decoration:none;color:inherit">${l}</a>`).join("")}</div>
      <div class="quick" style="grid-template-columns:1fr">
        <div class="q fav" data-go="favorites"><div class="cover">${ICONS.heartFill}</div>Favoritas</div>
        ${lib.playlists.map((p) => `<div class="q" data-go="playlist/${p.id}"><div class="cover"></div>${esc(p.name)}</div>`).join("")}
        <div class="q" data-act="new-playlist"><div class="cover" style="display:grid;place-items:center">${ICONS.plus}</div>Nueva playlist</div>
      </div>
      <h2>Álbumes</h2><div class="grid">${albumCards([...lib.albums.values()].sort((a, b) => a.name.localeCompare(b.name)))}</div>`;
    v.querySelectorAll("[data-go]").forEach((q) => (q.onclick = () => go(q.dataset.go)));
  },

  songs(v) {
    const sort = VIEWS.songs.sort || "artist";
    const sorters = {
      artist: null,
      title: (a, b) => a.title.localeCompare(b.title),
      recent: (a, b) => b.addedAt - a.addedAt,
      plays: (a, b) => b.playCount - a.playCount,
    };
    const tracks = sorters[sort] ? [...lib.tracks].sort(sorters[sort]) : lib.tracks;
    if (!tracks.length) return `<h1 class="page">Canciones</h1>${emptyLibrary()}`;
    v.innerHTML = `<h1 class="page">Canciones</h1>
      <p class="muted">${plural(tracks.length, "canción", "canciones")} · ${fmtLong(tracks.reduce((s, t) => s + t.duration, 0))}</p>
      ${playAllActions("Canciones")}
      <div class="chips">${[["artist", "Artista"], ["title", "Título"], ["recent", "Recientes"], ["plays", "Más escuchadas"]]
        .map(([k, l]) => `<button class="chip ${k === sort ? "active" : ""}" data-sort="${k}">${l}</button>`).join("")}</div>
      ${trackRows(tracks)}`;
    v.querySelectorAll("[data-sort]").forEach((b) => (b.onclick = () => { VIEWS.songs.sort = b.dataset.sort; render(); }));
    bindTrackList(v, tracks, "Tus canciones");
    return { tracks, source: "Tus canciones" };
  },

  albums(v) {
    const albums = [...lib.albums.values()].sort((a, b) => a.name.localeCompare(b.name));
    if (!albums.length) return `<h1 class="page">Álbumes</h1>${emptyLibrary()}`;
    v.innerHTML = `<h1 class="page">Álbumes</h1><div class="grid">${albumCards(albums)}</div>`;
  },

  artists(v) {
    const artists = [...lib.artists.values()].sort((a, b) => a.name.localeCompare(b.name));
    if (!artists.length) return `<h1 class="page">Artistas</h1>${emptyLibrary()}`;
    v.innerHTML = `<h1 class="page">Artistas</h1><div class="grid">${artistCards(artists)}</div>`;
  },

  favorites(v) {
    const tracks = lib.tracks.filter((t) => t.favorite);
    v.innerHTML = `<div class="hero"><div class="cover" style="background:linear-gradient(135deg,#450af5,#c4efd9);display:grid;place-items:center">${ICONS.heartFill.replace('width="24" height="24"', 'width="72" height="72"')}</div>
      <div><div class="kind">Playlist</div><h1>Favoritas</h1><div class="muted">${plural(tracks.length, "canción", "canciones")}</div></div></div>
      ${tracks.length ? playAllActions("Favoritas") + trackRows(tracks) : `<div class="empty"><h2>Aún no tienes favoritas</h2><p>Toca el corazón en cualquier canción para guardarla aquí.</p></div>`}`;
    bindTrackList(v, tracks, "Favoritas");
    return { tracks, source: "Favoritas" };
  },

  playlists(v) {
    v.innerHTML = `<h1 class="page">Playlists</h1>
      <div class="actions"><button class="btn" data-act="new-playlist">${ICONS.plus}Nueva playlist</button></div>
      ${lib.playlists.length ? `<div class="grid">${playlistCards(lib.playlists)}</div>` : `<div class="empty"><h2>Crea tu primera playlist</h2></div>`}`;
  },

  album(v) {
    const a = lib.albums.get(route.arg);
    if (!a) return go("albums");
    const tracks = [...a.tracks].sort((x, y) => (x.discNo ?? 0) - (y.discNo ?? 0) || (x.trackNo ?? 0) - (y.trackNo ?? 0));
    v.innerHTML = `<div class="hero">${coverImg(a.cover)}<div><div class="kind">Álbum</div><h1>${esc(a.name)}</h1>
      <div><a href="#/artist/${encodeURIComponent(a.artist)}" style="color:inherit;font-weight:700">${esc(a.artist)}</a>
      <span class="muted">${a.year ? " · " + a.year : ""} · ${plural(tracks.length, "canción", "canciones")}, ${fmtLong(tracks.reduce((s, t) => s + t.duration, 0))}</span></div></div></div>
      ${playAllActions(a.name, `<button class="icon-btn" data-act="add-all" title="Añadir a playlist">${ICONS.plus}</button>`)}
      ${trackRows(tracks, { noAlbum: true, noCover: true, numbers: "track" })}`;
    bindTrackList(v, tracks, a.name);
    return { tracks, source: a.name };
  },

  artist(v) {
    const a = lib.artists.get(route.arg);
    if (!a) return go("artists");
    const top = [...a.tracks].sort((x, y) => y.playCount - x.playCount);
    const albums = [...a.albums].map((k) => lib.albums.get(k)).filter(Boolean).sort((x, y) => (y.year ?? 0) - (x.year ?? 0));
    v.innerHTML = `<div class="hero">${coverImg(a.cover, "cover round")}<div><div class="kind">Artista</div><h1>${esc(a.name)}</h1>
      <div class="muted">${plural(a.tracks.length, "canción", "canciones")} · ${plural(albums.length, "álbum", "álbumes")}</div></div></div>
      ${playAllActions(a.name)}<h2>Canciones</h2>${trackRows(top)}
      <h2>Discografía</h2><div class="grid">${albumCards(albums)}</div>`;
    bindTrackList(v, top, a.name);
    return { tracks: top, source: a.name };
  },

  playlist(v) {
    const p = lib.playlists.find((x) => x.id === Number(route.arg));
    if (!p) return go("playlists");
    const tracks = p.trackIds.map((id) => lib.byId.get(id)).filter(Boolean);
    const first = tracks.find((t) => t.cover);
    v.innerHTML = `<div class="hero">${coverImg(first && first.cover)}<div><div class="kind">Playlist</div><h1>${esc(p.name)}</h1>
      <div class="muted">${plural(tracks.length, "canción", "canciones")}${tracks.length ? ", " + fmtLong(tracks.reduce((s, t) => s + t.duration, 0)) : ""}</div></div></div>
      ${playAllActions(p.name, `<button class="icon-btn" data-act="playlist-menu" title="Opciones">${ICONS.more}</button>`)}
      ${tracks.length ? trackRows(tracks) : `<div class="empty"><h2>Esta playlist está vacía</h2><p>Usa el menú ⋯ de cualquier canción para añadirla aquí.</p></div>`}`;
    bindTrackList(v, tracks, p.name, { playlistId: p.id });
    return { tracks, source: p.name };
  },

  settings(v) {
    const total = lib.tracks.reduce((s, t) => s + t.duration, 0);
    const imported = lib.tracks.filter((t) => t.imported).length;
    v.innerHTML = `<h1 class="page">Ajustes</h1>
      <div class="settings-card"><h3>Importar canciones</h3>
        <p>Copia archivos de audio dentro de la app. Quedan guardados en el almacenamiento privado de Mi Música y siempre se pueden escuchar sin conexión.</p>
        <button class="btn" data-act="import">${ICONS.upload}Elegir archivos</button></div>
      ${lib.isMobile ? "" : `<div class="settings-card"><h3>Carpetas de música</h3>
        <p>Las canciones se leen directamente de estas carpetas (no se copian). Re-escanea cuando añadas música nueva.</p>
        <div id="folders"></div>
        <div class="actions" style="margin-top:12px"><button class="btn" data-act="folder">${ICONS.folder}Añadir carpeta</button>
        <button class="btn ghost" data-act="rescan">${ICONS.refresh}Re-escanear</button></div></div>`}
      <div class="settings-card"><h3>Estadísticas</h3><div class="stats">
        <div><b>${lib.tracks.length}</b><span class="muted">canciones</span></div>
        <div><b>${lib.albums.size}</b><span class="muted">álbumes</span></div>
        <div><b>${lib.artists.size}</b><span class="muted">artistas</span></div>
        <div><b>${fmtLong(total)}</b><span class="muted">de música</span></div>
        <div><b>${imported}</b><span class="muted">importadas</span></div>
        <div><b>${lib.tracks.reduce((s, t) => s + t.playCount, 0)}</b><span class="muted">reproducciones</span></div>
      </div></div>
      <p class="muted" style="text-align:center">Mi Música 0.1 · hecha con Rust + Tauri · 100% offline</p>`;
    if (!lib.isMobile) renderFolders();
  },
};

bindCards($("#view"));

async function renderFolders() {
  const el = $("#folders");
  if (!el) return;
  lib.folders = await invoke("get_folders");
  el.innerHTML = lib.folders.length
    ? lib.folders.map((f) => `<div class="folder-row">${ICONS.folder}<span class="p">${esc(f.path)}</span>
        <button class="icon-btn" data-rm="${f.id}" title="Quitar carpeta">${ICONS.trash}</button></div>`).join("")
    : `<div class="muted">No hay carpetas todavía.</div>`;
  el.querySelectorAll("[data-rm]").forEach((b) => (b.onclick = async () => {
    if (!(await confirmSheet("¿Quitar esta carpeta? Sus canciones saldrán de la biblioteca (los archivos no se borran).", "Quitar"))) return;
    await invoke("remove_folder", { id: Number(b.dataset.rm) });
    await loadLibrary();
  }));
}

const TITLES = { home: "Inicio", search: "Buscar", library: "Tu biblioteca", songs: "Canciones", albums: "Álbumes", artists: "Artistas", favorites: "Favoritas", playlists: "Playlists", settings: "Ajustes" };
let currentList = null;

function render() {
  const v = $("#view");
  const fn = VIEWS[route.name] || VIEWS.home;
  currentList = null;
  const out = fn(v);
  if (typeof out === "string") v.innerHTML = out;
  else if (out && out.tracks) currentList = out;
  highlightNav();
  const h1 = v.querySelector("h1");
  $("#topbar-title").textContent = TITLES[route.name] || (h1 ? h1.textContent : "");
  $("#back-btn").style.visibility = ["album", "artist", "playlist"].includes(route.name) || (lib.isMobile && TAB_OF[route.name]) ? "visible" : "hidden";
}

// Acciones genéricas (botones con data-act dentro de la vista)
$("#view").addEventListener("click", (e) => {
  const b = e.target.closest("[data-act]");
  if (!b) return;
  const act = b.dataset.act;
  if (act === "import") $("#file-input").click();
  if (act === "folder") pickFolder();
  if (act === "rescan") rescan();
  if (act === "new-playlist") newPlaylist();
  if (act === "playlist-menu") playlistMenu(Number(route.arg));
  if (act === "shuffle") {
    player.setShuffle(!player.shuffle);
    b.classList.toggle("on", player.shuffle);
  }
  if (act === "play" && currentList) player.play(currentList.tracks.map((t) => t.id), null, currentList.source);
  if (act === "add-all" && currentList) addToPlaylistMenu(currentList.tracks.map((t) => t.id));
});
$("#view").addEventListener("scroll", () => $("#topbar").classList.toggle("scrolled", $("#view").scrollTop > 80), { passive: true });

// ============================================================================
// Controles del reproductor
// ============================================================================
function bindSeek(el) {
  el.addEventListener("input", () => {
    seeking = true;
    const frac = el.value / el.max;
    el.style.setProperty("--p", `${frac * 100}%`);
    $("#t-cur").textContent = $("#fp-cur").textContent = fmt(frac * (audio.duration || 0));
  });
  el.addEventListener("change", () => {
    seeking = false;
    if (audio.duration) audio.currentTime = (el.value / el.max) * audio.duration;
  });
}

function setupControls() {
  const btn = (id, icon, fn) => {
    const el = $(id);
    if (icon) el.innerHTML = ICONS[icon];
    el.addEventListener("click", (e) => { e.stopPropagation(); fn(e); });
  };
  btn("#btn-play", "play", () => player.toggle());
  btn("#fp-play", "play", () => player.toggle());
  btn("#btn-next", "next", () => player.next());
  btn("#fp-next", "next", () => player.next());
  btn("#btn-prev", "prev", () => player.prev());
  btn("#fp-prev", "prev", () => player.prev());
  btn("#btn-shuffle", "shuffle", () => player.setShuffle(!player.shuffle));
  btn("#fp-shuffle", "shuffle", () => player.setShuffle(!player.shuffle));
  btn("#btn-repeat", "repeat", () => player.cycleRepeat());
  btn("#fp-repeat", "repeat", () => player.cycleRepeat());
  btn("#btn-queue", "queue", showQueue);
  btn("#fp-queue", "queue", showQueue);
  btn("#np-fav", "heart", () => player.current && toggleFavorite(player.current.id));
  btn("#fp-fav", "heart", () => player.current && toggleFavorite(player.current.id));
  btn("#btn-mute", "volume", () => { audio.muted = !audio.muted; updateVolume(); });
  btn("#fp-close", "down", () => history.back());
  btn("#back-btn", "back", () => history.back());
  btn("#side-new-playlist", "plus", newPlaylist);

  bindSeek($("#seek"));
  bindSeek($("#fp-seek"));
  $("#volume").addEventListener("input", (e) => {
    audio.muted = false;
    audio.volume = e.target.value / 100;
    store.set("volume", audio.volume);
    updateVolume();
  });

  // En móvil, tocar el mini reproductor abre la pantalla completa.
  $("#now-open").addEventListener("click", () => {
    if (!lib.isMobile || !player.current || matchMedia("(min-width: 761px)").matches) return;
    $("#fullplayer").hidden = false;
    history.pushState({ fullplayer: true }, "");
  });
  window.addEventListener("popstate", () => {
    if (!$("#fullplayer").hidden) $("#fullplayer").hidden = true;
    if (!$("#sheet").hidden) closeSheet();
  });

  $("#file-input").addEventListener("change", (e) => {
    importFiles(e.target.files);
    e.target.value = "";
  });

  // Atajos de teclado en escritorio
  document.addEventListener("keydown", (e) => {
    if (e.target.matches("input:not([type=range])")) return;
    if (e.code === "Space") { e.preventDefault(); player.toggle(); }
    if (e.code === "ArrowRight" && e.ctrlKey) player.next();
    if (e.code === "ArrowLeft" && e.ctrlKey) player.prev();
    if (e.code === "Escape") closeSheet();
  });
}

// ============================================================================
// Inicio
// ============================================================================
async function init() {
  renderNav();
  setupControls();
  updateModes();
  updatePlayState();
  try {
    lib.isMobile = await invoke("is_mobile");
  } catch {}
  document.body.classList.toggle("mobile", lib.isMobile);
  parseRoute();
  try {
    await loadLibrary();
  } catch (e) {
    toast("Error cargando la biblioteca: " + e, 5000);
  }
  player.restore();
  updateNowPlaying();
}

init();
