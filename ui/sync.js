"use strict";
// ============================================================================
// Sincronización por Wi‑Fi (PC → teléfono)
// Se carga antes que app.js; usa sus utilidades ($, esc, invoke, go…) solo
// en tiempo de ejecución.
// ============================================================================

const fmtBytes = (b) => {
  if (b < 1024 * 1024) return `${Math.max(1, Math.round(b / 1024))} KB`;
  if (b < 1024 ** 3) return `${(b / 1024 ** 2).toFixed(b < 10 * 1024 ** 2 ? 1 : 0)} MB`;
  return `${(b / 1024 ** 3).toFixed(1)} GB`;
};

const syncUI = {
  step: "find", // find | code | pick | progress | done
  peers: [],
  searching: false,
  address: "",
  peerName: "",
  code: "",
  remote: null,
  tab: "albums",
  filter: "",
  sel: { albums: new Set(), playlists: new Set() },
  progress: null,
  report: null,
  error: "",
  connecting: false,
};

// --------------------------------------------------------------- lado de la PC

let sharePoll = null;

function shareCardHtml() {
  return `<div class="settings-card share-card" id="share-card"><h3>${ICONS.wifi} Compartir por Wi‑Fi</h3>
    <p>Permite que tu teléfono descargue música de esta PC. Ambos deben estar en la misma red Wi‑Fi.</p>
    <div id="share-body"></div></div>`;
}

async function renderShare(info) {
  const el = $("#share-body");
  if (!el) return;
  if (info === undefined) info = await invoke("share_status").catch(() => null);
  clearInterval(sharePoll);
  if (!info) {
    el.innerHTML = `<button class="btn" data-share="on">${ICONS.wifi}Activar</button>`;
  } else {
    const digits = info.code.split("").map((d) => `<span>${d}</span>`).join("");
    const addr = info.addresses.length ? info.addresses.map((a) => `${a}:${info.port}`).join(" · ") : `puerto ${info.port}`;
    el.innerHTML = `<div class="share-live">
        <div class="pulse-dot"></div><div><b>Compartiendo como «${esc(info.name)}»</b>
        <div class="muted">${info.sent ? `${plural(info.sent, "canción enviada", "canciones enviadas")}` : "Esperando a tu teléfono…"}</div></div></div>
      ${info.locked
        ? `<p class="warn">Se bloqueó por demasiados códigos incorrectos. Genera un código nuevo.</p>`
        : `<div class="muted small">En el teléfono: <b>Ajustes → Sincronizar con mi PC</b> y escribe este código</div>
           <div class="code-digits">${digits}</div>
           <div class="muted small">Si tu teléfono no encuentra la PC, escribe esta dirección: <b class="mono">${esc(addr)}</b></div>`}
      <div class="actions" style="margin:14px 0 0">
        <button class="btn ghost" data-share="on">${ICONS.refresh}Código nuevo</button>
        <button class="btn ghost" data-share="off">Desactivar</button></div>`;
    // Refrescamos el contador de canciones enviadas mientras se ve la tarjeta.
    sharePoll = setInterval(async () => {
      if (!$("#share-body")) return clearInterval(sharePoll);
      const s = await invoke("share_status").catch(() => null);
      const live = $("#share-body .share-live .muted");
      if (s && live && s.sent) live.textContent = plural(s.sent, "canción enviada", "canciones enviadas");
      if (s && s.locked !== info.locked) renderShare(s);
    }, 3000);
  }
  el.querySelectorAll("[data-share]").forEach((b) => (b.onclick = async () => {
    try {
      if (b.dataset.share === "on") renderShare(await invoke("share_start"));
      else {
        await invoke("share_stop");
        renderShare(null);
      }
    } catch (e) {
      toast(String(e), 4000);
    }
  }));
}

function syncCardHtml() {
  return `<div class="settings-card"><h3>${ICONS.download} Sincronizar con ${lib.isMobile ? "mi PC" : "otra PC"}</h3>
    <p>Elige álbumes o playlists de tu PC y descárgalos aquí por Wi‑Fi. Después se escuchan sin conexión.</p>
    <a class="btn" href="#/sync" style="text-decoration:none">${ICONS.wifi}Sincronizar</a></div>`;
}

// ---------------------------------------------------------- lado del teléfono

async function syncSearch() {
  syncUI.searching = true;
  syncUI.error = "";
  if (route.name === "sync") render();
  try {
    syncUI.peers = await invoke("sync_discover");
  } catch (e) {
    syncUI.peers = [];
  }
  syncUI.searching = false;
  if (route.name === "sync" && syncUI.step === "find") render();
}

async function syncConnect() {
  const input = $("#sync-code");
  syncUI.code = (input ? input.value : syncUI.code).replace(/\D/g, "");
  if (syncUI.code.length !== 6) return toast("El código tiene 6 dígitos", 2000);
  if (syncUI.connecting) return;
  syncUI.connecting = true;
  syncUI.error = "";
  const btn = $("#sync-connect");
  if (btn) { btn.disabled = true; btn.textContent = "Conectando…"; }
  try {
    syncUI.remote = await invoke("sync_connect", { address: syncUI.address, code: syncUI.code });
    syncUI.peerName = syncUI.remote.name;
    syncUI.sel = { albums: new Set(), playlists: new Set() };
    syncUI.step = "pick";
  } catch (e) {
    syncUI.error = String(e);
    if (syncUI.step !== "code") toast(syncUI.error, 4000);
  }
  syncUI.connecting = false;
  render();
}

function remoteAlbums() {
  const map = new Map();
  for (const t of syncUI.remote.tracks) {
    const k = `${t.albumArtist}\u0001${t.album}`;
    let a = map.get(k);
    if (!a) map.set(k, (a = { key: k, name: t.album, artist: t.albumArtist, year: t.year, tracks: [] }));
    a.tracks.push(t);
    a.year ||= t.year;
  }
  return [...map.values()].sort((a, b) => a.name.localeCompare(b.name));
}

/** Canciones elegidas (álbumes + playlists) que aún no están en el teléfono. */
function syncSelection() {
  const byId = new Map(syncUI.remote.tracks.map((t) => [t.id, t]));
  const ids = new Set();
  for (const a of remoteAlbums()) if (syncUI.sel.albums.has(a.key)) a.tracks.forEach((t) => ids.add(t.id));
  for (const p of syncUI.remote.playlists) if (syncUI.sel.playlists.has(p.id)) p.trackIds.forEach((id) => ids.add(id));
  const missing = [...ids].map((id) => byId.get(id)).filter((t) => t && !t.onDevice);
  return { missing, bytes: missing.reduce((s, t) => s + t.size, 0) };
}

function groupRow(kind, id, title, subtitle, tracks) {
  const fresh = tracks.filter((t) => !t.onDevice);
  const selected = kind === "album" ? syncUI.sel.albums.has(id) : syncUI.sel.playlists.has(id);
  const status = !tracks.length
    ? `<span class="badge">Vacía</span>`
    : !fresh.length
      ? `<span class="badge ok">${ICONS.check}Ya la tienes</span>`
      : fresh.length < tracks.length
        ? `<span class="badge">${plural(fresh.length, "nueva", "nuevas")}</span>`
        : "";
  return `<div class="pick ${selected ? "on" : ""}" data-pick="${kind}" data-id="${esc(id)}">
    <span class="check">${ICONS.check}</span>
    <div class="cover"></div>
    <div class="pick-text"><div class="t ellipsis">${esc(title)}</div>
      <div class="muted ellipsis small">${esc(subtitle)} · ${plural(tracks.length, "canción", "canciones")} · ${fmtBytes(tracks.reduce((s, t) => s + t.size, 0) || 1)}</div></div>
    ${status}</div>`;
}

function updateSyncBar() {
  const bar = $("#sync-bar");
  if (!bar) return;
  const { missing, bytes } = syncSelection();
  const anyPlaylist = syncUI.sel.playlists.size > 0;
  bar.querySelector(".sum").innerHTML = missing.length
    ? `<b>${plural(missing.length, "canción nueva", "canciones nuevas")}</b><span class="muted"> · ${fmtBytes(bytes)}</span>`
    : anyPlaylist || syncUI.sel.albums.size
      ? `<span class="muted">Ya tienes todo lo elegido${anyPlaylist ? " (se actualizarán las playlists)" : ""}</span>`
      : `<span class="muted">Elige qué quieres descargar</span>`;
  bar.querySelector("button").disabled = !missing.length && !anyPlaylist;
}

function syncView(v) {
  const s = syncUI;
  const head = `<h1 class="page">Sincronizar</h1>`;

  if (s.step === "progress" || s.step === "done") return syncProgressView(v, head);

  if (s.step === "find") {
    v.innerHTML = `${head}
      <div class="sync-steps"><div><b>1</b><span>En tu PC abre BlueWhale → <b>Ajustes → Compartir por Wi‑Fi</b> → Activar.</span></div>
      <div><b>2</b><span>Conecta este ${lib.isMobile ? "teléfono" : "equipo"} a la <b>misma red Wi‑Fi</b>.</span></div></div>
      <h2>PCs encontradas</h2>
      <div class="peers">${s.searching
        ? `<div class="searching"><span class="sonar-mini"></span>Buscando tu PC en la red…</div>`
        : s.peers.length
          ? s.peers.map((p) => `<button class="peer" data-addr="${esc(p.address)}" data-name="${esc(p.name)}">${ICONS.computer}
              <span><b>${esc(p.name)}</b><br><small class="muted">${esc(p.address)}</small></span>${ICONS.chevron}</button>`).join("")
          : `<div class="muted">No encontré ninguna PC. Comprueba que «Compartir por Wi‑Fi» esté activado.</div>`}</div>
      <div class="actions"><button class="btn ghost" data-sync="search" ${s.searching ? "disabled" : ""}>${ICONS.refresh}Buscar de nuevo</button></div>
      <h2>O escribe la dirección</h2>
      <form class="inline-form" id="sync-manual"><input id="sync-addr" placeholder="Ej. 192.168.1.20:47810" inputmode="url" autocomplete="off" value="${esc(s.address)}" />
        <button class="btn" type="submit">Siguiente</button></form>
      <p class="muted small">La dirección aparece en la PC, debajo del código.</p>`;
    v.querySelectorAll(".peer").forEach((b) => (b.onclick = () => {
      s.address = b.dataset.addr;
      s.peerName = b.dataset.name;
      s.step = "code";
      render();
    }));
    $("#sync-manual", v).onsubmit = (e) => {
      e.preventDefault();
      const a = $("#sync-addr", v).value.trim();
      if (!a) return toast("Escribe la dirección de tu PC", 2000);
      s.address = a;
      s.peerName = a;
      s.step = "code";
      render();
    };
    v.querySelector('[data-sync="search"]').onclick = syncSearch;
    return;
  }

  if (s.step === "code") {
    v.innerHTML = `${head}
      <div class="settings-card code-card">${ICONS.computer}<h3>${esc(s.peerName)}</h3>
        <p>Escribe el código de 6 dígitos que aparece en tu PC.</p>
        <form id="sync-code-form"><input id="sync-code" class="code-input" inputmode="numeric" maxlength="6" autocomplete="one-time-code" placeholder="••••••" value="${esc(s.code)}" />
        ${s.error ? `<p class="warn">${esc(s.error)}</p>` : ""}
        <div class="actions" style="justify-content:center"><button type="button" class="btn ghost" data-sync="back">Cambiar de PC</button>
        <button class="btn" id="sync-connect" type="submit">Conectar</button></div></form></div>`;
    const input = $("#sync-code", v);
    input.focus();
    input.oninput = () => {
      input.value = input.value.replace(/\D/g, "").slice(0, 6);
      if (input.value.length === 6) syncConnect();
    };
    $("#sync-code-form", v).onsubmit = (e) => { e.preventDefault(); syncConnect(); };
    v.querySelector('[data-sync="back"]').onclick = () => { s.step = "find"; s.error = ""; render(); };
    return;
  }

  // ---- elegir qué descargar
  const albums = remoteAlbums();
  const f = norm(s.filter);
  const albumsShown = albums.filter((a) => !f || norm(`${a.name} ${a.artist}`).includes(f));
  const byId = new Map(s.remote.tracks.map((t) => [t.id, t]));
  const lists = s.remote.playlists.filter((p) => !f || norm(p.name).includes(f));
  const totalNew = s.remote.tracks.filter((t) => !t.onDevice).length;

  v.innerHTML = `${head}
    <div class="connected">${ICONS.computer}<div><b>${esc(s.remote.name)}</b>
      <div class="muted small">${plural(s.remote.tracks.length, "canción", "canciones")} · ${totalNew ? `${totalNew} que aún no tienes` : "ya lo tienes todo"}</div></div>
      <button class="btn ghost small-btn" data-sync="disconnect">Desconectar</button></div>
    <div class="chips">
      <button class="chip ${s.tab === "albums" ? "active" : ""}" data-tab="albums">Álbumes · ${albums.length}</button>
      <button class="chip ${s.tab === "playlists" ? "active" : ""}" data-tab="playlists">Playlists · ${s.remote.playlists.length}</button>
      <button class="chip" data-sync="all-new">Elegir todo lo nuevo</button>
      ${s.sel.albums.size || s.sel.playlists.size ? `<button class="chip" data-sync="none">Quitar selección</button>` : ""}
    </div>
    <div class="search-box">${ICONS.search}<input id="sync-filter" type="search" placeholder="Filtrar…" value="${esc(s.filter)}" autocomplete="off" /></div>
    <div class="picks">${s.tab === "albums"
      ? albumsShown.map((a) => groupRow("album", a.key, a.name, a.artist + (a.year ? ` · ${a.year}` : ""), a.tracks)).join("") || `<div class="muted">Sin resultados</div>`
      : lists.map((p) => groupRow("playlist", p.id, p.name, "Playlist", p.trackIds.map((id) => byId.get(id)).filter(Boolean))).join("") || `<div class="muted">${s.filter ? "Sin resultados" : "Tu PC no tiene playlists"}</div>`}</div>
    <div class="sync-bar" id="sync-bar"><div class="sum"></div><button class="btn" data-sync="download">${ICONS.download}Descargar</button></div>`;

  updateSyncBar();
  v.querySelectorAll("[data-tab]").forEach((b) => (b.onclick = () => { s.tab = b.dataset.tab; render(); }));
  const filter = $("#sync-filter", v);
  filter.oninput = () => {
    s.filter = filter.value;
    const pos = filter.selectionStart;
    render();
    const again = $("#sync-filter");
    again.focus();
    again.setSelectionRange(pos, pos);
  };
  v.querySelector(".picks").onclick = (e) => {
    const row = e.target.closest("[data-pick]");
    if (!row) return;
    const set = row.dataset.pick === "album" ? s.sel.albums : s.sel.playlists;
    const id = row.dataset.pick === "album" ? row.dataset.id : Number(row.dataset.id);
    set.has(id) ? set.delete(id) : set.add(id);
    row.classList.toggle("on", set.has(id));
    updateSyncBar();
  };
  v.querySelector('[data-sync="all-new"]').onclick = () => {
    for (const a of albums) if (a.tracks.some((t) => !t.onDevice)) s.sel.albums.add(a.key);
    render();
  };
  const none = v.querySelector('[data-sync="none"]');
  if (none) none.onclick = () => { s.sel = { albums: new Set(), playlists: new Set() }; render(); };
  v.querySelector('[data-sync="disconnect"]').onclick = () => { s.remote = null; s.code = ""; s.step = "find"; render(); syncSearch(); };
  v.querySelector('[data-sync="download"]').onclick = syncDownload;
}

async function syncDownload() {
  const albums = remoteAlbums().filter((a) => syncUI.sel.albums.has(a.key));
  const trackIds = albums.flatMap((a) => a.tracks.map((t) => t.id));
  const { missing, bytes } = syncSelection();
  syncUI.progress = { done: 0, total: missing.length, current: "", bytesDone: 0, bytesTotal: bytes, failed: 0 };
  syncUI.report = null;
  syncUI.error = "";
  try {
    await invoke("sync_start", { trackIds, playlistIds: [...syncUI.sel.playlists] });
    syncUI.step = "progress";
  } catch (e) {
    toast(String(e), 4000);
  }
  render();
}

function syncProgressView(v, head) {
  const s = syncUI;
  const p = s.progress || { done: 0, total: 0, bytesDone: 0, bytesTotal: 0, current: "" };
  const frac = p.bytesTotal ? Math.min(1, p.bytesDone / p.bytesTotal) : s.step === "done" ? 1 : 0;
  if (s.step === "progress") {
    v.innerHTML = `${head}<div class="settings-card sync-progress">
      <div class="dive"><div class="dive-whale"></div><div class="dive-water" style="height:${Math.round(frac * 100)}%"></div></div>
      <h3 id="sp-pct">${Math.round(frac * 100)}%</h3>
      <div class="muted" id="sp-count">${p.done} de ${p.total} canciones · ${fmtBytes(p.bytesDone || 1)} de ${fmtBytes(p.bytesTotal || 1)}</div>
      <div class="progress-bar"><div id="sp-bar" style="width:${frac * 100}%"></div></div>
      <div class="ellipsis small" id="sp-current">${esc(p.current || "Preparando…")}</div>
      <p class="muted small">Mantén BlueWhale abierta y el Wi‑Fi conectado hasta que termine.</p>
      <button class="btn ghost" data-sync="cancel">Cancelar</button></div>`;
    v.querySelector('[data-sync="cancel"]').onclick = async (e) => {
      e.target.disabled = true;
      e.target.textContent = "Cancelando…";
      await invoke("sync_cancel");
    };
    return;
  }
  const r = s.report || { downloaded: 0, skipped: 0, failed: [], playlists: 0, cancelled: false };
  const title = s.error ? "No se pudo completar" : r.cancelled ? "Descarga cancelada" : r.failed.length ? "Terminado con algunos errores" : "¡Todo listo!";
  v.innerHTML = `${head}<div class="settings-card sync-progress">
    <div class="whale-big"></div><h3>${title}</h3>
    ${s.error ? `<p class="warn">${esc(s.error)}</p>` : ""}
    <div class="stats">
      <div><b>${r.downloaded}</b><span class="muted">descargadas</span></div>
      <div><b>${r.skipped}</b><span class="muted">ya las tenías</span></div>
      ${r.playlists ? `<div><b>${r.playlists}</b><span class="muted">playlists</span></div>` : ""}
      ${r.failed.length ? `<div><b>${r.failed.length}</b><span class="muted">con error</span></div>` : ""}
    </div>
    ${r.failed.length ? `<details class="small"><summary>Ver errores</summary>${r.failed.map((f) => `<div class="muted">${esc(f)}</div>`).join("")}</details>` : ""}
    <div class="actions" style="justify-content:center">
      ${s.remote ? `<button class="btn ghost" data-sync="more">Elegir más</button>` : ""}
      <a class="btn" href="#/home" style="text-decoration:none">Ir a mi música</a></div></div>`;
  const more = v.querySelector('[data-sync="more"]');
  if (more) more.onclick = () => syncConnect();
}

function onSyncProgress(p) {
  syncUI.progress = p;
  if (route.name !== "sync" || syncUI.step !== "progress") return;
  const frac = p.bytesTotal ? Math.min(1, p.bytesDone / p.bytesTotal) : 0;
  const pct = `${Math.round(frac * 100)}%`;
  $("#sp-pct").textContent = pct;
  $("#sp-bar").style.width = pct;
  $(".dive-water").style.height = pct;
  $("#sp-count").textContent = `${p.done} de ${p.total} canciones · ${fmtBytes(p.bytesDone || 1)} de ${fmtBytes(p.bytesTotal || 1)}`;
  $("#sp-current").textContent = p.current || "Preparando…";
}

async function onSyncFinished(report, error) {
  syncUI.report = report;
  syncUI.error = error || "";
  syncUI.step = "done";
  await loadLibrary();
  if (route.name === "sync") render();
  else {
    toast(error ? `Sincronización: ${error}` : `Sincronización terminada: ${plural(report.downloaded, "canción nueva", "canciones nuevas")}`, 4000);
  }
}

function setupSyncEvents() {
  const ev = window.__TAURI__ && window.__TAURI__.event;
  if (!ev) return;
  ev.listen("sync-progress", (e) => onSyncProgress(e.payload));
  ev.listen("sync-finished", (e) => onSyncFinished(e.payload, ""));
  ev.listen("sync-error", (e) => onSyncFinished(null, String(e.payload)));
}

/** Se llama cada vez que se entra en #/sync. */
function enterSync() {
  // Tras terminar, volver a entrar recarga el catálogo (para ver lo nuevo).
  if (syncUI.step === "done") {
    if (syncUI.remote) return syncConnect();
    syncUI.step = "find";
  }
  if (syncUI.step === "find" && !syncUI.searching && !syncUI.peers.length) syncSearch();
}
