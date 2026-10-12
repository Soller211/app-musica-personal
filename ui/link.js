"use strict";
// ============================================================================
// Importar desde enlace: pega un enlace directo a un audio (o a un ZIP con un
// álbum) y BlueWhale lo descarga a la biblioteca. Se carga antes que app.js.
// ============================================================================

const linkUI = { busy: false, progress: null };

function linkSheet() {
  if (linkUI.busy) return renderLinkProgress();
  openSheet(
    `<h3>${ICONS.link} Importar desde enlace</h3>
     <form id="link-form">
       <p class="muted small" style="margin:0">Pega un enlace <b>directo</b> a un archivo de audio (MP3, M4A, FLAC, OGG, WAV…)
       o a un <b>ZIP</b> con un álbum, como las descargas de Bandcamp o Internet Archive.</p>
       <div class="link-row"><input id="link-input" type="url" inputmode="url" placeholder="https://…" autocomplete="off" />
         <button type="button" class="btn ghost small-btn" data-act="paste">Pegar</button></div>
       <div class="row"><button type="button" class="btn ghost" data-act="no">Cancelar</button>
         <button class="btn" type="submit">${ICONS.download}Descargar</button></div>
     </form>`,
    async (act) => {
      if (act === "no") return closeSheet();
      if (act === "paste") {
        try {
          $("#link-input").value = (await navigator.clipboard.readText()).trim();
        } catch {
          toast("Mantén pulsado el campo y elige «Pegar»", 2500);
          $("#link-input").focus();
        }
      }
    }
  );
  const input = $("#link-input");
  input.focus();
  $("#link-form").onsubmit = (e) => {
    e.preventDefault();
    const url = input.value.trim();
    if (!/^https?:\/\//i.test(url)) return toast("Pega un enlace que empiece por http:// o https://", 2500);
    startLinkImport(url);
  };
}

async function startLinkImport(url) {
  linkUI.busy = true;
  linkUI.progress = { name: "", bytesDone: 0, bytesTotal: null };
  $("#toast").hidden = true;
  renderLinkProgress();
  try {
    const tracks = await invoke("import_url", { url });
    await loadLibrary();
    closeSheet();
    toast(
      tracks.length === 1
        ? `«${tracks[0].title}» añadida a tu biblioteca`
        : `${plural(tracks.length, "canción añadida", "canciones añadidas")} a tu biblioteca`,
      3500
    );
  } catch (e) {
    closeSheet();
    toast(String(e), 5000);
  } finally {
    linkUI.busy = false;
    $("#sheet-backdrop").onclick = closeSheet;
  }
}

function renderLinkProgress() {
  openSheet(
    `<h3>${ICONS.download} Descargando</h3>
     <div class="link-progress">
       <div class="ellipsis" id="lp-name">Conectando…</div>
       <div class="progress-bar"><div id="lp-bar" style="width:0%"></div></div>
       <div class="muted small" id="lp-bytes"></div>
       <div class="row"><button type="button" class="btn ghost" data-act="cancel">Cancelar</button></div>
     </div>`,
    async (act, b) => {
      if (act !== "cancel") return;
      b.disabled = true;
      b.textContent = "Cancelando…";
      await invoke("import_url_cancel");
    }
  );
  // Mientras descarga, tocar fuera no cierra la hoja (se puede cancelar con el botón).
  $("#sheet-backdrop").onclick = null;
  if (linkUI.progress) onLinkProgress(linkUI.progress);
}

function onLinkProgress(p) {
  linkUI.progress = p;
  const name = $("#lp-name");
  if (!name) return;
  name.textContent = p.name || "Conectando…";
  const bar = $("#lp-bar");
  if (p.bytesTotal) {
    const pct = Math.min(100, Math.round((p.bytesDone / p.bytesTotal) * 100));
    bar.style.width = `${pct}%`;
    bar.classList.remove("indeterminate");
    $("#lp-bytes").textContent = `${pct}% · ${fmtBytes(p.bytesDone || 1)} de ${fmtBytes(p.bytesTotal)}`;
  } else {
    bar.classList.add("indeterminate");
    $("#lp-bytes").textContent = p.bytesDone ? fmtBytes(p.bytesDone) : "";
  }
}

function setupLinkEvents() {
  const ev = window.__TAURI__ && window.__TAURI__.event;
  if (ev) ev.listen("link-progress", (e) => onLinkProgress(e.payload));
}
