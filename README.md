<p align="center"><img src="assets/icon.svg" width="120" alt="BlueWhale"></p>

# 🐋 BlueWhale

Tu océano de música personal: reproduce **tu** música, **100% offline**, en el
teléfono (Android / iOS) y en el PC (Windows / macOS / Linux).

Está escrita en **Rust** con [Tauri 2](https://tauri.app). Rust se encarga de la
biblioteca (leer metadatos y carátulas, base de datos SQLite, playlists,
favoritos, estadísticas) y la interfaz es HTML/CSS/JS sin dependencias, así que
no hace falta internet ni ningún servidor.

## Diseño

BlueWhale tiene identidad propia, inspirada en el océano profundo:

- Paleta de **azules abisales** con acento **aqua bioluminiscente** y corazones en coral
- Logo de **ballena** (`assets/icon.svg`), que también aparece en las carátulas vacías
- Tipografía **Outfit** incluida en la app (`ui/fonts/`, licencia SIL OFL), así se ve igual sin internet
- **Burbujas** que suben por el fondo y **olas** animadas en el inicio
- La canción que suena muestra un **ecualizador** animado
- En el teléfono, la carátula se ve en una **portilla** redonda que gira mientras suena, con ondas de **sonar**
- Respeta la opción del sistema "reducir movimiento"

## Funciones

- Biblioteca con **Canciones, Álbumes, Artistas, Favoritas y Playlists**
- **Buscador** instantáneo (ignora acentos y mayúsculas)
- Lee título, artista, álbum, nº de pista, año y **carátula** de tus archivos
- Reproductor con **aleatorio, repetir (todo / una), cola, "reproducir a continuación"**
- Reproductor a **pantalla completa** en el móvil, mini‑reproductor y barra inferior
- **Inicio** con lo escuchado recientemente, lo añadido y lo que más escuchas
- Recuerda la cola y la posición al cerrar la app
- Controles del sistema (Media Session): teclas multimedia en el PC
- Atajos: `Espacio` reproducir/pausa, `Ctrl+→` / `Ctrl+←` siguiente/anterior
- Formatos: MP3, FLAC, OGG, Opus, M4A/AAC, WAV, AIFF

### ¿Cómo meto mi música?

| Dónde | Cómo |
|---|---|
| **Teléfono** | *Ajustes → Importar canciones*: eliges archivos y se **copian dentro de la app**, así siempre están disponibles sin conexión. |
| **PC** | *Ajustes → Añadir carpeta*: la app lee tu carpeta de música directamente (no copia nada). Usa *Re‑escanear* cuando añadas canciones. También puedes importar archivos sueltos. |

## Sincronizar por Wi‑Fi (PC → teléfono)

Pasa música de tu PC al teléfono sin cables, eligiendo qué quieres:

1. En la **PC**: *Ajustes → Compartir por Wi‑Fi → Activar*. Aparece un **código de 6 dígitos**.
2. En el **teléfono** (en la misma red Wi‑Fi): *Ajustes → Sincronizar con mi PC*.
   La app busca tu PC sola; si no la encuentra, escribe la dirección que se ve en la PC
   (por ejemplo `192.168.1.20:47810`).
3. Escribe el código, **elige álbumes o playlists** y pulsa *Descargar*.

- Solo baja lo que **aún no tienes**; las playlists elegidas se crean (o actualizan) en el teléfono.
- Lo descargado queda en el teléfono y se escucha **sin conexión**.
- El servidor de la PC solo funciona mientras "Compartir" está activado y solo en tu red local.
  Tras 20 códigos incorrectos se bloquea hasta que generes un código nuevo.
- La primera vez, **Windows o macOS pueden preguntar si permites conexiones entrantes**:
  acepta para redes **privadas**.
- Puertos usados: `47809/UDP` (descubrimiento) y `47810/TCP` (descarga).
- Las redes públicas o de empresa a veces bloquean la comunicación entre dispositivos;
  en casa normalmente funciona sin tocar nada.

## Estructura del proyecto

```
crates/bluewhale-core/   Núcleo en Rust (sin interfaz): SQLite, escaneo, metadatos, playlists,
                      sincronización Wi‑Fi (sync/) + tests
src-tauri/            App Tauri: expone el núcleo a la interfaz como comandos
ui/                   Interfaz (index.html, style.css, app.js, sync.js, icons.js)
assets/icon.svg       Icono original (genera todos los iconos con `npm run icons`)
.github/workflows/    CI: tests + compilación del APK de Android y de los instaladores de PC
```

Los datos de la app (base de datos, canciones importadas y carátulas) se guardan
en la carpeta de datos de la app de cada sistema, por ejemplo
`~/.local/share/com.soller.bluewhale/` en Linux o
`%APPDATA%\com.soller.bluewhale\` en Windows.

## Requisitos

1. **Rust** (estable): <https://rustup.rs>
2. **Node.js 18+** (solo para la CLI de Tauri)
3. Dependencias del sistema según tu plataforma:
   <https://tauri.app/start/prerequisites/>
   - Linux: `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev`
   - Windows: WebView2 (viene con Windows 10/11) y "Build Tools for Visual Studio"
   - Android: Android Studio (SDK + NDK) y Java 17
   - iOS: macOS con Xcode

```bash
npm install
```

## Ejecutar en el PC

```bash
npm run dev      # modo desarrollo
npm run build    # crea el instalador en target/release/bundle/
```

## Instalar en el teléfono (Android)

```bash
# una sola vez: añade los targets de Rust y genera el proyecto Android
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
npm run android:init
npm run icons

# con el teléfono conectado por USB (depuración USB activada)
npm run android:dev

# o crea un APK
npx tauri android build --apk --debug
```

El APK queda en `src-tauri/gen/android/app/build/outputs/apk/`. Cópialo al
teléfono y ábrelo para instalarlo (permite "instalar apps de origen
desconocido").

> 💡 **Sin instalar nada en tu PC**: en GitHub ve a *Actions → Build → Run
> workflow*. Al terminar, descarga el artefacto **bluewhale-android** (el APK) o
> los instaladores de escritorio. También se compilan automáticamente al crear un
> tag `v*` (por ejemplo `v0.1.0`).

### iOS

```bash
npm run ios:init
npm run ios:dev
```

Requiere un Mac con Xcode y una cuenta de Apple (gratuita para instalar en tu
propio iPhone). Nota: Safari/iOS no reproduce OGG; usa MP3, M4A/AAC o FLAC.

## Tests

```bash
cargo test --workspace
```

## Limitaciones conocidas / ideas futuras

- La reproducción la hace el motor web del sistema (WebView), por eso los
  formatos soportados dependen de él. En Linux, para FLAC/OGG necesitas los
  plugins de GStreamer (`gstreamer1.0-plugins-good`).
- En Android, la reproducción con la pantalla bloqueada / app en segundo plano
  depende del WebView; para un control total (notificación con controles) el
  siguiente paso sería un plugin nativo de Tauri con un `MediaSessionService`.
- Ideas: ecualizador, letras (`.lrc`), editar etiquetas, sincronizar también
  favoritas y playlists del teléfono hacia la PC.
