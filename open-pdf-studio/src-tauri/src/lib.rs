// The MCP `tools/list` descriptor table in mcp_server.rs is one large
// `serde_json::json!` literal; its recursive macro expansion exceeds the
// default limit of 128. The 98-tool public catalog requires a limit of 512.
#![recursion_limit = "512"]

mod accounts;
mod systeem_lettertypen;
mod pdf_beveiliging;
mod atomic_file;
pub mod bestand_openen;
pub mod batch_ocr;
pub mod batch_ocr_walk;
pub mod beveiligde_pdf;
pub mod beheerd;
pub mod cad_export;
pub mod cad_import;
pub mod crashlog;
pub mod datamap;
pub mod draagbaar;
mod email;
mod installed_fonts;
pub mod export_policy;
pub mod extensie_catalogus;
pub mod extensie_installatie;
pub mod linux_runtime;
pub mod logboek;
pub mod mcp_app_bridge;
pub mod mcp_extensie_tools;
pub mod mcp_koppeling;
pub mod mcp_server;
pub mod mcp_signaal;
pub mod mcp_tool_meta;
pub mod monochroom;
pub mod verscherpen;
pub mod symbool_zoeken;
pub mod openaec_omgeving;
// OCR (Tesseract) is desktop only; Android has no Tesseract build.
pub mod ocr;
pub mod pdfium_renderer;
pub mod pdfx_cmyk;
pub mod handtekening;
pub mod headless;
pub mod herstel;
pub mod laagweergave;
pub mod instantie;
pub mod papierformaten;
pub mod print_instelling;
pub mod print_duplex;
pub mod print_plaatsing;
// Of de app de standaard-PDF-app is: xdg-mime en de sandbox-status.
pub mod standaard_app;
// Printen: rechtstreeks met CUPS via IPP, met lp/lpstat en het printportaal
// als terugval (print_unix.rs).
pub mod cups_ipp;
pub mod print_unix;
pub mod render_to_png;
pub mod spraak;
// Scanner input (#382): SANE.
pub mod scanner;
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod scanner_cmd;
pub mod shared_review;
pub mod central_review_transport;
pub mod central_review;
pub mod window_mgmt;
pub mod startup_diagnostics;
mod startup_visibility;
mod startup_window;
pub mod worker_pool;

pub struct StartupOpts {
    pub mcp_server: bool,
    pub mcp_port: u16,
}

impl Default for StartupOpts {
    fn default() -> Self {
        Self { mcp_server: false, mcp_port: 9223 }
    }
}

use std::fs;
use std::fs::File;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use tauri::{Emitter, Manager};
use tauri_plugin_fs::FsExt;

// Store the file paths passed via command line
struct OpenedFiles(Mutex<Vec<String>>);

// Store locked file handles to prevent other apps from writing
struct LockedFiles(Mutex<HashMap<String, File>>);

// Cache PDF file bytes to avoid re-reading from disk on every render call
struct PdfBytesCache(Mutex<HashMap<String, Vec<u8>>>);

// Cache parsed DocumentHandle objects so the font registry inside each
// handle survives across page renders. The handle holds a Mutex<FontRegistry>
// internally — extracting glyph outlines for a font (the dominant cost on
// text-heavy pages) only runs the first time that font is encountered in
// the document. Without this cache every Tauri command would create a
// fresh DocumentHandle and re-extract every glyph from scratch.
struct DocHandleCache(Mutex<HashMap<String, Arc<open_pdf_render::DocumentHandle>>>);

/// Gebouwde TileScenes voor het zware-blad-tegelpad (route A): één scene is
/// de complete display-list + chunk-index (~140-300 MB op extreme CAD-bladen)
/// en vervangt de ~1,1 GB PDFium-parse-state per worker. Cap 2: actief blad +
/// vergelijk-/vorig blad. Key: pad|mtime|len|pagina|rotatie.
struct TileSceneCache(Mutex<Vec<(String, Arc<open_pdf_render::tile_render::TileScene>)>>);
const TILE_SCENE_CACHE_CAP: usize = 2;

/// Cache for already-rendered thumbnails. Keyed by (path, page, max_width, rotation).
/// Hits return instantly without touching the renderer or JPEG encoder, which
/// makes scrolling back to previously-rendered pages essentially free.
struct ThumbnailCache(Mutex<HashMap<(String, u32, u32, i32), String>>);

/// Cache for `analyze_page_type` results. Keyed by (path, page_index). The
/// underlying lopdf classifier is cheap on most pages but for construction
/// PDFs with multi-megabyte content streams the size-shortcut in
/// `analyze_page_type` keeps it fast even cold; this cache makes warm hits
/// effectively free (HashMap lookup) so per-page navigation has zero analyze
/// cost after the first visit (or after `analyze_page_type_batch` warms it).
struct PageTypeCache(Mutex<HashMap<(String, u32), String>>);

#[tauri::command]
fn get_opened_file(state: tauri::State<OpenedFiles>) -> Vec<String> {
    state.0.lock().unwrap().clone()
}

/// Welk document opent een `RunEvent::Opened`-URL, als het een PDF is?
///
/// - `file://` (macOS Finder, iOS): het bestandspad, alleen voor een `.pdf`.
/// - `content://` (Android, #593): de URI zelf. "Openen met" vanuit een
///   bestandsbeheerder of mailapp levert geen pad maar een content-URI, vaak
///   zonder `.pdf` aan het eind (`.../document/msf%3A1234`). Het intent-filter
///   dat tauri-build uit `bundle.fileAssociations` maakt, laat alleen
///   `application/pdf` door; de fs-plugin leest de URI via de ContentResolver.
///   Op het bureaublad komt een content-URI nooit voor en telt hij niet.
///
/// Andere schema's (de deep-link-plugin, https) laten we met rust.
fn geopend_pdf_document(url: &url::Url, android: bool) -> Option<String> {
    match url.scheme() {
        "file" => url
            .to_file_path()
            .ok()
            .map(|p| p.to_string_lossy().to_string())
            .filter(|p| p.to_lowercase().ends_with(".pdf")),
        "content" if android => Some(url.as_str().to_string()),
        _ => None,
    }
}

/// Welk PDF-bestand opent een opdrachtregelargument?
///
/// Windows en de meeste starters geven een pad door. Een Linux-starter met
/// `Exec=open-pdf-studio %U` (GNOME/Cinnamon "Openen met", #748) geeft een
/// `file://`-URI, met procenttekens voor spaties en andere tekens; die
/// wordt een pad. Opties (`-...`) en andere schema's tellen niet.
fn pdf_uit_argument(arg: &str) -> Option<String> {
    if arg.starts_with('-') {
        return None;
    }
    // `get` en geen `[..7]`: byte 7 kan midden in een UTF-8-teken vallen
    // (bijv. "ééééé.pdf"), en dan zou de app bij de start in paniek raken.
    if arg.len() > 7 && arg.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("file://")) {
        return url::Url::parse(arg).ok().and_then(|u| geopend_pdf_document(&u, false));
    }
    // Een deep link of web-URL is geen bestand (de deep-link-plugin doet die).
    if arg.contains("://") {
        return None;
    }
    arg.to_lowercase().ends_with(".pdf").then(|| arg.to_string())
}

#[tauri::command]
fn get_session_file_path() -> String {
    if let Some(data_dir) = datamap::lokale_datamap() {
        let app_dir = data_dir.join("OpenPDFStudio");
        if !app_dir.exists() {
            let _ = fs::create_dir_all(&app_dir);
        }
        app_dir.join("session.json").to_string_lossy().to_string()
    } else {
        "session.json".to_string()
    }
}

#[tauri::command]
fn save_session(data: String) -> Result<bool, String> {
    let path = get_session_file_path();
    fs::write(&path, data).map_err(|e| e.to_string())?;
    Ok(true)
}

#[tauri::command]
fn load_session() -> Option<String> {
    let path = get_session_file_path();
    fs::read_to_string(&path).ok()
}

pub(crate) fn get_preferences_file_path() -> String {
    if let Some(data_dir) = datamap::lokale_datamap() {
        let app_dir = data_dir.join("OpenPDFStudio");
        if !app_dir.exists() {
            let _ = fs::create_dir_all(&app_dir);
        }
        app_dir.join("preferences.json").to_string_lossy().to_string()
    } else {
        "preferences.json".to_string()
    }
}

#[tauri::command]
fn save_preferences(data: String) -> Result<bool, String> {
    let path = get_preferences_file_path();
    fs::write(&path, data).map_err(|e| e.to_string())?;
    Ok(true)
}

#[tauri::command]
fn load_preferences() -> Option<String> {
    let path = get_preferences_file_path();
    fs::read_to_string(&path).ok()
}

/// Voorkeur "Vloeiend scrollen" (#1007) live naar elke WebView van dit proces.
/// Alleen WebKitGTK (Linux) heeft daar een instelling voor; WebView2 en
/// WKWebView niet, daar regelt de JS-kant het wielscrollen zelf. Geeft terug
/// of de instelling is doorgegeven.
#[tauri::command]
fn zet_vloeiend_scrollen(app: tauri::AppHandle, aan: bool) -> bool {
    {
        for venster in app.webview_windows().values() {
            linux_runtime::zet_vloeiend_scrollen::<tauri::Wry>(venster.as_ref(), aan);
        }
        true
    }
}

// Symboolcatalogi als losse bestanden naast preferences.json (#354): grote
// gedownloade collecties horen niet in het instellingenbestand — dat wordt
// bij elke voorkeurswijziging integraal herschreven en de localStorage-
// spiegel kent een quotum. Kleine catalogi blijven gewoon inline in de
// voorkeuren; alleen grote landen hier (beide opties bestaan naast elkaar).
fn get_catalogs_dir() -> std::path::PathBuf {
    let basis = datamap::lokale_datamap().unwrap_or_else(|| std::path::PathBuf::from("."));
    let dir = basis.join("OpenPDFStudio").join("catalogs");
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
    dir
}

/// Catalogus-id -> veilige bestandsnaam: alleen alfanumeriek, '-', '_' en
/// '.'; al het andere wordt '_'. Voorkomt padtrucs via het id.
fn veilige_catalogus_bestandsnaam(id: &str) -> String {
    let schoon: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect();
    let schoon = schoon.trim_matches('.').to_string();
    let schoon = if schoon.is_empty() { "catalogus".to_string() } else { schoon };
    format!("{}.json", schoon)
}

// Atomair (tijdelijk bestand + hernoemen, atomic_file.rs): een afgebroken
// schrijfactie laat de vorige catalogus heel in plaats van een half bestand.
#[tauri::command]
fn save_catalog(id: String, data: String) -> Result<bool, String> {
    let path = get_catalogs_dir().join(veilige_catalogus_bestandsnaam(&id));
    atomic_file::write(&path, data.as_bytes()).map_err(|e| e.to_string())?;
    Ok(true)
}

#[tauri::command]
fn load_catalog(id: String) -> Option<String> {
    let path = get_catalogs_dir().join(veilige_catalogus_bestandsnaam(&id));
    fs::read_to_string(&path).ok()
}

#[tauri::command]
fn delete_catalog(id: String) -> Result<bool, String> {
    let path = get_catalogs_dir().join(veilige_catalogus_bestandsnaam(&id));
    match fs::remove_file(&path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

/// True when a package manager installs the updates (Flatpak/Flathub). The
/// built-in updater is then not registered, and Help > Check for updates says
/// where updates come from instead.
#[tauri::command]
fn updates_managed_externally() -> bool {
    linux_runtime::running_in_flatpak()
}

#[tauri::command]
fn get_username() -> String {
    whoami::username()
}

// Font catalogue queries may touch OS caches; keep them off the UI thread.
#[tauri::command]
async fn list_installed_font_families() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(installed_fonts::installed_font_families)
        .await
        .map_err(|error| format!("Font enumeration task failed: {error}"))?
}

// Fallback commands for when plugins aren't available via global API

#[tauri::command]
fn write_file(path: String, data: String) -> Result<bool, String> {
    let bytes = BASE64.decode(&data).map_err(|e| e.to_string())?;
    atomic_file::write(std::path::Path::new(&path), &bytes).map_err(|e| e.to_string())?;
    Ok(true)
}

// Writes that can still be cancelled (#766), by the id from the
// x-opds-cancel-id header. A cancel for an id that has not started yet is kept,
// so the write stops before it begins.
type SchrijfAnnuleringen = std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<std::sync::atomic::AtomicBool>>>;
static SCHRIJF_ANNULERINGEN: std::sync::OnceLock<SchrijfAnnuleringen> = std::sync::OnceLock::new();

fn schrijf_annuleringen() -> std::sync::MutexGuard<'static, std::collections::HashMap<String, std::sync::Arc<std::sync::atomic::AtomicBool>>> {
    SCHRIJF_ANNULERINGEN.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

fn schrijf_annulering(id: &str) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    schrijf_annuleringen().entry(id.to_string()).or_default().clone()
}

/// Cancel a running `write_file_atomic` before it replaces its destination.
#[tauri::command]
fn cancel_file_write(id: String) {
    if id.is_empty() || id.len() > 64 { return; }
    let mut lijst = schrijf_annuleringen();
    // A cancel that arrived just after its write finished leaves an entry
    // nobody uses; keep the list small.
    if lijst.len() > 64 {
        lijst.retain(|_, vlag| std::sync::Arc::strong_count(vlag) > 1);
    }
    lijst.entry(id).or_default().store(true, std::sync::atomic::Ordering::SeqCst);
}

#[tauri::command]
async fn write_file_atomic(request: tauri::ipc::Request<'_>) -> Result<(), String> {
    let header = request.headers().get("x-opds-path")
        .ok_or("Missing destination path")?.to_str().map_err(|e| e.to_string())?;
    let path = urlencoding::decode(header).map_err(|e| e.to_string())?.into_owned();
    let cancel_id = request.headers().get("x-opds-cancel-id")
        .and_then(|v| v.to_str().ok()).filter(|v| !v.is_empty() && v.len() <= 64).map(str::to_string);
    let bytes = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes.clone(),
        // Android cannot transport a raw IPC body.
        tauri::ipc::InvokeBody::Json(value) => {
            serde_json::from_value::<Vec<u8>>(value.get("data").cloned().ok_or("Missing file data")?)
                .map_err(|e| e.to_string())?
        }
    };
    let Some(id) = cancel_id else {
        return tauri::async_runtime::spawn_blocking(move || {
            atomic_file::write(std::path::Path::new(&path), &bytes).map_err(|e| e.to_string())
        }).await.map_err(|e| e.to_string())?;
    };
    let cancelled = schrijf_annulering(&id);
    let result = tauri::async_runtime::spawn_blocking(move || {
        atomic_file::write_cancellable(std::path::Path::new(&path), &bytes, &cancelled).map_err(|e| e.to_string())
    }).await.map_err(|e| e.to_string());
    schrijf_annuleringen().remove(&id);
    result?
}

#[tauri::command]
async fn write_file_new(request: tauri::ipc::Request<'_>) -> Result<(), String> {
    let header = request.headers().get("x-opds-path")
        .ok_or("Missing destination path")?.to_str().map_err(|e| e.to_string())?;
    let path = urlencoding::decode(header).map_err(|e| e.to_string())?.into_owned();
    let bytes = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes.clone(),
        // Android cannot transport a raw IPC body.
        tauri::ipc::InvokeBody::Json(value) => {
            serde_json::from_value::<Vec<u8>>(value.get("data").cloned().ok_or("Missing file data")?)
                .map_err(|e| e.to_string())?
        }
    };
    tauri::async_runtime::spawn_blocking(move || {
        atomic_file::write_new(std::path::Path::new(&path), &bytes).map_err(|e| e.to_string())
    }).await.map_err(|e| e.to_string())?
}

// A component extracted from a PDF collection: untrusted bytes. Never replaces
// a file, refuses autostart folders (and, with x-opds-restrict, programs and
// scripts) and marks the result as downloaded content on Windows.
#[tauri::command]
async fn write_downloaded_file_new(request: tauri::ipc::Request<'_>) -> Result<(), String> {
    let header = request.headers().get("x-opds-path")
        .ok_or("Missing destination path")?.to_str().map_err(|e| e.to_string())?;
    let path = urlencoding::decode(header).map_err(|e| e.to_string())?.into_owned();
    let restrict = request.headers().get("x-opds-restrict").map_or(true, |v| v.as_bytes() != b"0");
    if let Some(code) = export_policy::problem(&path, restrict) {
        return Err(code.to_string());
    }
    let bytes = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes.clone(),
        tauri::ipc::InvokeBody::Json(value) => {
            serde_json::from_value::<Vec<u8>>(value.get("data").cloned().ok_or("Missing file data")?)
                .map_err(|e| e.to_string())?
        }
    };
    tauri::async_runtime::spawn_blocking(move || {
        let target = std::path::Path::new(&path);
        atomic_file::write_new(target, &bytes).map_err(|e| e.to_string())?;
        export_policy::mark_of_the_web(target).map_err(|e| e.to_string())
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn file_exists(path: String) -> bool {
    std::path::Path::new(&path).exists()
}

#[tauri::command]
async fn open_url(url: String) -> Result<(), String> {
    headless::weiger("open_url")?;
    open::that(&url).map_err(|e| e.to_string())
}

#[tauri::command]
fn is_dev_mode() -> bool {
    cfg!(debug_assertions)
}

/// What is known about the default handler for .pdf files, and whether this
/// app can make itself the default (see standaard_app::StandaardAppStatus).
/// Blocking: runs a registry read on Windows and `xdg-mime` on Linux.
fn standaard_app_status() -> standaard_app::StandaardAppStatus {
    // Beheerde modus (OPENAEC_BEHEERD=1): de omgeving regelt de koppeling;
    // niet in het register of met xdg-mime kijken, en nooit om de standaard-
    // app vragen.
    if beheerd::is_beheerd() {
        return standaard_app::beheerd_status();
    }

    {
        // xdg-mime works on any XDG-compliant desktop (GNOME, KDE, Cinnamon,
        // MATE, XFCE) across distros. A Flatpak or Snap has no xdg-mime and
        // no write access to the desktop's associations: then the status says
        // so and the UI explains where to choose the default instead.
        let sandbox = linux_runtime::sandbox();
        let query = if sandbox.is_some() || !print_unix::programma_aanwezig("xdg-mime") {
            None
        } else {
            std::process::Command::new("xdg-mime")
                .args(["query", "default", "application/pdf"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        };
        standaard_app::linux_status(sandbox, query.as_deref())
    }

}

/// Check if this app is the default handler for .pdf files.
/// Returns true if our exe/desktop entry is the registered handler, false
/// otherwise (also when it cannot be checked; use default_pdf_app_status to
/// tell those apart). Uses spawn_blocking so the subprocess calls never block
/// the main thread.
#[tauri::command]
async fn is_default_pdf_app() -> bool {
    tauri::async_runtime::spawn_blocking(|| standaard_app_status().is_default == Some(true))
        .await
        .unwrap_or(false)
}

/// The default-PDF-app status for the startup bar and the File Associations
/// tab: { isDefault: bool|null, current: string|null, canSet: bool,
/// reason: "flatpak"|"snap"|"xdg-mime-missing"|"unsupported"|null }.
#[tauri::command]
async fn default_pdf_app_status() -> standaard_app::StandaardAppStatus {
    tauri::async_runtime::spawn_blocking(standaard_app_status)
        .await
        .unwrap_or(standaard_app::StandaardAppStatus {
            is_default: None,
            current: None,
            can_set: false,
            reason: Some("unsupported"),
        })
}

/// Make this app the default handler for .pdf files.
/// On Windows, opens the system default-apps settings page. On Linux, sets the
/// xdg-mime default directly (no system UI for this exists on most desktops).
#[tauri::command]
fn open_default_apps_settings() -> Result<bool, String> {
    headless::weiger("open_default_apps_settings")?;

    {
        // Inside a Flatpak or Snap there is no xdg-mime and the host's mime
        // associations cannot be written; the desktop's own settings choose
        // the default there. The UI checks default_pdf_app_status first and
        // explains that; this error is the backstop.
        if let Some(sandbox) = linux_runtime::sandbox() {
            return Err(format!(
                "Running as a {sandbox} package: choose Open PDF Studio as the default PDF app in your desktop's settings (Default Applications), or via \"Open with\" on a PDF file."
            ));
        }
        // xdg-mime is the desktop-environment agnostic way to set the default
        // handler. The .desktop filename must match what tauri-bundler wrote in
        // /usr/share/applications (the bundle uses the app's productName).
        let status = std::process::Command::new("xdg-mime")
            .args(&["default", "Open PDF Studio.desktop", "application/pdf"])
            .status()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    "xdg-mime not found. Install xdg-utils (e.g. `sudo apt install xdg-utils`) and try again.".to_string()
                } else {
                    format!("Failed to run xdg-mime: {}", e)
                }
            })?;
        if !status.success() {
            return Err(format!(
                "xdg-mime exited with status {}. The .desktop file may not be registered — install Open PDF Studio via the bundled .deb/.AppImage so /usr/share/applications/Open PDF Studio.desktop exists.",
                status
            ));
        }
        Ok(true)
    }

}

/// Lock a file to prevent other applications from writing to it.
/// Opens the file with shared read access only (no write sharing on Windows).
#[tauri::command]
fn lock_file(path: String, state: tauri::State<LockedFiles>) -> Result<bool, String> {
    let mut locks = state.0.lock().map_err(|e| e.to_string())?;

    // Already locked by us
    if locks.contains_key(&path) {
        return Ok(true);
    }

    // On Unix this is only an open handle: best-effort, like before.
    let file = bestand_openen::vergrendel(&path).map_err(|e| format!("Failed to lock file: {}", e))?;
    locks.insert(path, file);
    Ok(true)
}

/// Uitkomst van het vergrendelen bij het openen van een document (#1005).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentVergrendeling {
    #[serde(flatten)]
    modus: bestand_openen::OpenModus,
    /// De fout van het vergrendelen, als dat mislukte.
    fout: Option<bestand_openen::OpenFout>,
    pad_info: bestand_openen::PadInfo,
}

/// Lees een document om te openen (#1005): met delen voor lezen, schrijven en
/// verwijderen, zonder de fs-scope (die een WebDAV-pad niet altijd kan
/// canonicaliseren), en op een eigen thread, want een plaatshouder die alleen
/// online staat wordt bij het lezen eerst opgehaald. Een fout zegt welk pad,
/// welke stap en welke soort (bestand_openen::OpenFout).
#[tauri::command]
async fn read_document_file(path: String) -> Result<tauri::ipc::Response, bestand_openen::OpenFout> {
    let pad = path.clone();
    let data = tauri::async_runtime::spawn_blocking(move || {
        bestand_openen::lees_document(&pad, &bestand_openen::sync_wortels_uit_omgeving())
    })
    .await
    .map_err(|e| bestand_openen::OpenFout::van_io(
        bestand_openen::OpenStap::Lezen,
        &path,
        &std::io::Error::other(e.to_string()),
        false,
        bestand_openen::classificeer_pad(&path, &[]),
    ))??;
    Ok(tauri::ipc::Response::new(data))
}

/// Vergrendel een zojuist gelezen document en besluit of het bewerkbaar of
/// alleen-lezen opent (#1005). Mislukt het vergrendelen (de
/// synchronisatie-app of een andere app heeft het bestand open om te
/// schrijven, of het mag niet), dan opent het alleen-lezen in plaats van niet.
#[tauri::command]
async fn lock_document_file(path: String, state: tauri::State<'_, LockedFiles>) -> Result<DocumentVergrendeling, String> {
    let wortels = bestand_openen::sync_wortels_uit_omgeving();
    let pad_info = bestand_openen::classificeer_pad(&path, &wortels);
    if state.0.lock().map_err(|e| e.to_string())?.contains_key(&path) {
        let modus = bestand_openen::beslis_openmodus(false, bestand_openen::Vergrendeling::Gelukt);
        return Ok(DocumentVergrendeling { modus, fout: None, pad_info });
    }
    let pad = path.clone();
    let (kenmerken, poging) = tauri::async_runtime::spawn_blocking(move || {
        let kenmerken = bestand_openen::kenmerken(&pad).unwrap_or(0);
        // Alleen-lezen bestanden hoeven geen vergrendeling: de app schrijft er
        // niet naar, en de synchronisatie-app blijft ze kunnen bijwerken.
        if kenmerken & bestand_openen::FILE_ATTRIBUTE_READONLY != 0 {
            return (kenmerken, None);
        }
        (kenmerken, Some(bestand_openen::vergrendel(&pad)))
    })
    .await
    .map_err(|e| e.to_string())?;
    let plaatshouder = bestand_openen::is_plaatshouder(kenmerken);
    let alleen_lezen_kenmerk = kenmerken & bestand_openen::FILE_ATTRIBUTE_READONLY != 0;
    let (vergrendeling, fout) = match poging {
        None => (bestand_openen::Vergrendeling::NietGeprobeerd, None),
        Some(Ok(file)) => {
            state.0.lock().map_err(|e| e.to_string())?.insert(path.clone(), file);
            (bestand_openen::Vergrendeling::Gelukt, None)
        }
        Some(Err(e)) => {
            let f = bestand_openen::OpenFout::van_io(bestand_openen::OpenStap::Vergrendelen, &path, &e, plaatshouder, pad_info.clone());
            (bestand_openen::Vergrendeling::Mislukt(f.soort), Some(f))
        }
    };
    let modus = bestand_openen::beslis_openmodus(alleen_lezen_kenmerk, vergrendeling);
    Ok(DocumentVergrendeling { modus, fout, pad_info })
}

/// Unlock a previously locked file, allowing other apps to write to it.
#[tauri::command]
fn unlock_file(path: String, state: tauri::State<LockedFiles>) -> Result<bool, String> {
    let mut locks = state.0.lock().map_err(|e| e.to_string())?;
    // Removing the entry drops the File handle, releasing the lock
    locks.remove(&path);
    Ok(true)
}


/// Enumerate installed printers.
/// Returns a JSON array of printer objects: Name, DriverName, PortName,
/// Default and PrinterStatus, the shape of Win32_Printer.
/// Alleen opvragen: er wordt niets geïnstalleerd, gewijzigd of gestart.
/// PortName hoort erbij omdat de printdialoog en `app_list_printers` daaraan
/// zien of een wachtrij naar een bestand schrijft (js/pdf/print-doel.js,
/// `isBestandsPrinter`).
/// Linux: CUPS over IPP, `lpstat` as the fallback, and inside a Flatpak/Snap
/// without any printer an entry for the desktop's print dialog.
/// async: the query runs on a blocking thread, so a slow printer server never
/// holds up window show and input processing at startup.
#[tauri::command]
async fn get_printers() -> Result<String, String> {
    // CUPS over IPP on its socket first, `lpstat` when that fails, and inside
    // a Flatpak/Snap without any printer an entry for the desktop's print
    // dialog (print_unix.rs). The JSON keeps the Win32-printer shape:
    // { Name, DriverName, PortName, Default, PrinterStatus }.
    let sandbox = linux_runtime::sandbox().is_some();
    tauri::async_runtime::spawn_blocking(move || print_unix::lijst_printers(sandbox))
        .await
        .map_err(|e| format!("Failed to enumerate printers: {}", e))?

}

/// Print a PDF file to a specific printer.
///
/// The JS side rasterises each page into a temp PDF at the right size and
/// rotation; CUPS then only has to take one document for one queue.
/// `plaatsing`: absent or anything but "vel" = each page fitted and centred
/// in the printable area (the behaviour before this parameter); "vel" = the
/// Print dialog has already laid every page out at paper size with the
/// chosen scale and position, so each page goes 1:1 onto the physical sheet
/// (see print_plaatsing.rs and js/pdf/print-plaatsing.js).
/// `kleur`: the colour mode of the Print dialog ('color' | 'grayscale' |
/// 'bw'). The page images already carry it; CUPS additionally gets
/// print-color-mode=monochrome for the non-colour modes, so a colour printer
/// does not use colour ink for grey. Absent = printer default.
/// async for the same reason as get_printers: spooling is slow blocking work
/// and runs on a blocking thread, not on the main event-loop thread.
#[tauri::command]
async fn print_pdf(
    path: String,
    printer: String,
    orientatie: Option<String>,
    papier: Option<String>,
    plaatsing: Option<String>,
    kleur: Option<String>,
    duplex: Option<String>,
) -> Result<bool, String> {
    // Keuzes uit de Pagina-instelling. Zonder argumenten: Auto + printerstandaard,
    // precies het gedrag van vóór deze parameters.
    let orientatie = print_instelling::Orientatie::uit_keuze(orientatie.as_deref());
    let papier = print_instelling::Papier::uit_keuze(papier.as_deref());
    let plaatsing = print_plaatsing::Plaatsing::uit_keuze(plaatsing.as_deref());
    let duplex = print_duplex::Duplex::parse(duplex.as_deref())?;
    // The JS side has already rasterised the selected pages into a temp PDF at
    // the right size and rotation (with plaatsing "vel": at paper size, scale
    // and position already applied), and calls this once per copy — so CUPS
    // only has to take one document for one queue and needs no page-range or
    // copy options of its own. Sent as an IPP Print-Job over the CUPS socket,
    // else with `lp`, else (Flatpak/Snap) through the print portal; the
    // options are the same `lp -o` values on every route (print_unix.rs).
    {
        let mut opties = print_instelling::lp_opties(orientatie, papier, plaatsing);
        duplex.append_cups(&mut opties);
        if let Some(k) = print_unix::kleur_optie(kleur.as_deref()) {
            opties.push("-o".to_string());
            opties.push(k);
        }
        let sandbox = linux_runtime::sandbox().is_some();
        tauri::async_runtime::spawn_blocking(move || {
            print_unix::print_bestand(std::path::Path::new(&path), &printer, &opties, sandbox, kleur.as_deref())
        })
        .await
        .map_err(|e| format!("Print task failed: {e}"))??;
        Ok(true)
    }

}

/// Open the document properties (printing preferences) dialog for a printer.
///
/// CUPS has no per-driver properties dialog that an application can call, so
/// the system's own printer settings for that queue are opened instead and
/// the command returns null.
#[tauri::command]
async fn open_printer_properties(
    window: tauri::WebviewWindow,
    printer: String,
    papier: Option<String>,
    orientatie: Option<String>,
) -> Result<Option<print_instelling::EigenschappenKeuze>, String> {
    headless::weiger("printer properties dialog")?;
    // The system's own printer settings for that queue. Each candidate is
    // tried in turn; the first one that launches wins.
    let _ = (window, papier, orientatie);
    let queue = printer.trim();

    // CUPS queue names cannot contain spaces or slashes, so the name is safe
    // to drop into the local CUPS URL unencoded.
    let candidates: Vec<(&str, Vec<String>)> = vec![
        ("system-config-printer", vec![format!("--printer={}", queue)]),
        ("xdg-open", vec![format!("http://localhost:631/printers/{}", queue)]),
    ];

    for (program, args) in candidates {
        if std::process::Command::new(program).args(&args).spawn().is_ok() {
            return Ok(None);
        }
    }

    Err("Could not open the system printer settings".to_string())
}

/// CUPS reports the paper of the queue itself; the Print dialog uses that and
/// the command has nothing to add. Never shows UI, never starts a job and
/// never changes anything.
#[tauri::command]
async fn printer_papier(printer: String, papier: Option<String>) -> Result<Option<print_instelling::PapierInfo>, String> {
    let _ = (printer, papier);
    Ok(None)
}

/// CUPS does not report the unprintable margins per job, so the Print dialog
/// works with a zero margin, as before.
#[tauri::command]
async fn printer_bedrukbaar(printer: String, papier: Option<String>) -> Result<Option<print_plaatsing::Bedrukbaar>, String> {
    let _ = (printer, papier);
    Ok(None)
}

/// Het pad van de meegeleverde voorbeeldtekening: een plattegrond op A3,
/// schaal 1:50, met een meetschaal in het bestand. De Open-pane en de
/// welkomstbalk gebruiken hem om meteen iets te kunnen meten en markeren.
/// `None` als de resource ontbreekt (een build zonder `resources/voorbeeld`).
#[tauri::command]
fn voorbeeld_tekening_pad(app: tauri::AppHandle) -> Option<String> {
    let map = app.path().resource_dir().ok()?.join("voorbeeld");
    let pad = map.join("voorbeeld-plattegrond.pdf");
    pad.is_file().then(|| pad.to_string_lossy().into_owned())
}

/// Get the system temp directory path.
#[tauri::command]
fn get_temp_dir() -> String {
    std::env::temp_dir().to_string_lossy().to_string()
}

/// Maak van een documentnaam een veilige bestandsnaam (verboden tekens weg,
/// niet leeg, begrensd in lengte).
fn sanitize_temp_pdf_name(name: &str) -> String {
    let vervangen: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    vervangen
        .trim()
        .trim_matches('.')
        .trim()
        .chars()
        .take(120)
        .collect()
}

/// Write binary data to a temp file and return the path.
/// This bypasses the FS plugin scope restrictions.
///
/// Met `name` krijgt het tempbestand de (opgeschoonde) documentnaam, in een
/// unieke submap: de printspooler (Windows én CUPS) toont de bestandsnaam als
/// jobnaam, dus zo heet de printjob naar het pdf-bestand in plaats van naar
/// een timestamp.
///
/// The PDF travels as the raw IPC body and the name percent-encoded in the
/// `x-opds-name` header, so a large print job is not turned into a JSON array
/// of numbers on its way here.
#[tauri::command]
fn write_temp_pdf(request: tauri::ipc::Request<'_>) -> Result<String, String> {
    let mut name = match request.headers().get("x-opds-name") {
        Some(v) => Some(urlencoding::decode(v.to_str().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?.into_owned()),
        None => None,
    };
    let data = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => std::borrow::Cow::Borrowed(bytes.as_slice()),
        // Android cannot transport a raw IPC body.
        tauri::ipc::InvokeBody::Json(value) => {
            if name.is_none() {
                name = value.get("name").and_then(|n| n.as_str()).map(str::to_string);
            }
            std::borrow::Cow::Owned(
                serde_json::from_value::<Vec<u8>>(value.get("data").cloned().ok_or("Missing file data")?)
                    .map_err(|e| e.to_string())?,
            )
        }
    };
    let temp_dir = std::env::temp_dir();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let schoon = name
        .as_deref()
        .map(sanitize_temp_pdf_name)
        .filter(|s| !s.is_empty());
    let path = match schoon {
        Some(mut n) => {
            if !n.to_lowercase().ends_with(".pdf") {
                n.push_str(".pdf");
            }
            let job_dir = temp_dir.join(format!("openstudio_print_{}", timestamp));
            fs::create_dir_all(&job_dir)
                .map_err(|e| format!("Failed to create temp dir: {}", e))?;
            job_dir.join(n)
        }
        None => temp_dir.join(format!("openstudio_print_{}.pdf", timestamp)),
    };
    fs::write(&path, data.as_ref()).map_err(|e| format!("Failed to write temp file: {}", e))?;
    Ok(path.to_string_lossy().to_string())
}

/// Delete a file at the given path.
#[tauri::command]
fn delete_file(path: String) -> Result<bool, String> {
    fs::remove_file(&path).map_err(|e| format!("Failed to delete file: {}", e))?;
    Ok(true)
}

/// Rename (move) a file from old_path to new_path.
#[tauri::command]
fn rename_file(old_path: String, new_path: String) -> Result<bool, String> {
    fs::rename(&old_path, &new_path).map_err(|e| format!("Failed to rename file: {}", e))?;
    Ok(true)
}

/// Open a file in the system's default PDF viewer. Used by the auto-open
/// hook after a successful Save As so the user immediately sees their
/// freshly-saved file in their preferred reader.
///
/// `xdg-open "<path>"` honours the `application/pdf` MIME default.
#[tauri::command]
fn open_pdf_in_default_viewer(path: String) -> Result<bool, String> {
    headless::weiger("open_pdf_in_default_viewer")?;
    if !std::path::Path::new(&path).exists() {
        return Err(format!("File not found: {}", path));
    }
    {
        std::process::Command::new("xdg-open")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("Failed to launch default PDF viewer: {}", e))?;
        Ok(true)
    }
}

/// Reveal a file in the desktop's file manager.
#[tauri::command]
fn reveal_in_file_manager(path: String) -> Result<bool, String> {
    headless::weiger("reveal_in_file_manager")?;
    if !std::path::Path::new(&path).exists() {
        return Err(format!("File not found: {}", path));
    }
    {
        // No universal "select file" verb across file managers; open the
        // containing directory via xdg-open (works on every desktop).
        let parent = std::path::Path::new(&path)
            .parent()
            .ok_or_else(|| "Bestand heeft geen bovenliggende map".to_string())?;
        std::process::Command::new("xdg-open")
            .arg(parent)
            .spawn()
            .map_err(|e| format!("Failed to open file manager: {}", e))?;
        Ok(true)
    }
}

/// Download a PDF from a URL and save it to a temp file.
/// Returns the temp file path on success.
#[tauri::command]
async fn download_pdf_from_url(url: String) -> Result<String, String> {
    let response = reqwest::get(&url).await.map_err(|e| format!("Download failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("HTTP error: {}", response.status()));
    }

    let bytes = response.bytes().await.map_err(|e| format!("Failed to read response: {}", e))?;

    if bytes.is_empty() {
        return Err("Downloaded file is empty".to_string());
    }

    let temp_dir = std::env::temp_dir();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    // Try to extract filename from URL
    let filename = url.split('/').last()
        .and_then(|s| s.split('?').next())
        .filter(|s| s.to_lowercase().ends_with(".pdf"))
        .unwrap_or("download.pdf");

    let path = temp_dir.join(format!("openstudio_url_{}_{}", timestamp, filename));
    fs::write(&path, &bytes).map_err(|e| format!("Failed to write temp file: {}", e))?;
    Ok(path.to_string_lossy().to_string())
}

/// List PDF files in a directory.
/// Returns a list of full file paths for .pdf files found.
#[tauri::command]
fn list_pdf_files(dir: String) -> Result<Vec<String>, String> {
    let entries = fs::read_dir(&dir).map_err(|e| format!("Failed to read directory: {}", e))?;
    let mut pdf_files: Vec<String> = Vec::new();

    for entry in entries {
        if let Ok(entry) = entry {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    if ext.to_string_lossy().to_lowercase() == "pdf" {
                        pdf_files.push(path.to_string_lossy().to_string());
                    }
                }
            }
        }
    }

    pdf_files.sort();
    Ok(pdf_files)
}

/// PDF files directly in `dir` with size and modification time, so a folder
/// watch can skip unchanged files without reading them.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PdfFileStat {
    path: String,
    size: u64,
    mtime_ms: f64,
}

#[tauri::command]
fn list_pdf_file_stats(dir: String) -> Result<Vec<PdfFileStat>, String> {
    let entries = fs::read_dir(&dir).map_err(|e| format!("Failed to read directory: {}", e))?;
    let mut files: Vec<PdfFileStat> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_pdf = path.extension().map_or(false, |e| e.to_string_lossy().eq_ignore_ascii_case("pdf"));
        let Ok(meta) = fs::metadata(&path) else { continue };
        if !is_pdf || !meta.is_file() {
            continue;
        }
        let mtime_ms = meta.modified().ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        files.push(PdfFileStat { path: path.to_string_lossy().to_string(), size: meta.len(), mtime_ms });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

#[tauri::command]
fn play_alert_sound() {
    {
        std::process::Command::new("paplay")
            .arg("/usr/share/sounds/freedesktop/stereo/dialog-warning.oga")
            .spawn()
            .ok();
    }
}

// --- Plugin Management ---

fn get_plugins_dir() -> std::path::PathBuf {
    let data_dir = datamap::lokale_datamap().unwrap_or_else(|| std::path::PathBuf::from("."));
    let plugins_dir = data_dir.join("OpenPDFStudio").join("plugins");
    if !plugins_dir.exists() {
        let _ = fs::create_dir_all(&plugins_dir);
    }
    plugins_dir
}

#[tauri::command]
fn list_plugins() -> Result<Vec<serde_json::Value>, String> {
    let plugins_dir = get_plugins_dir();
    let mut plugins = Vec::new();

    if let Ok(entries) = fs::read_dir(&plugins_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let manifest_path = path.join("plugin.json");
                if manifest_path.exists() {
                    if let Ok(content) = fs::read_to_string(&manifest_path) {
                        if let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&content) {
                            plugins.push(manifest);
                        }
                    }
                }
            }
        }
    }

    Ok(plugins)
}

#[tauri::command]
fn install_plugin(path: String) -> Result<serde_json::Value, String> {
    let src_path = std::path::Path::new(&path);
    if !src_path.exists() {
        return Err("File not found".to_string());
    }

    let file = fs::File::open(&src_path).map_err(|e| format!("Failed to open file: {}", e))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("Invalid plugin archive: {}", e))?;

    // Read plugin.json from the archive to get the plugin id
    let manifest_content = {
        let mut manifest_file = archive.by_name("plugin.json")
            .map_err(|_| "Plugin archive must contain plugin.json".to_string())?;
        let mut content = String::new();
        std::io::Read::read_to_string(&mut manifest_file, &mut content)
            .map_err(|e| format!("Failed to read plugin.json: {}", e))?;
        content
    };

    let manifest: serde_json::Value = serde_json::from_str(&manifest_content)
        .map_err(|e| format!("Invalid plugin.json: {}", e))?;

    let plugin_id = manifest.get("id")
        .and_then(|v| v.as_str())
        .ok_or("plugin.json must have an 'id' field")?;

    // Strikte id-controle (#550): "" of "." wees naar de pluginmap zelf en
    // "C:" ontsnapte via Path::join.
    extensie_installatie::valideer_extensie_id(plugin_id).map_err(|f| f.message)?;

    let plugins_dir = get_plugins_dir();
    let plugin_dir = plugins_dir.join(plugin_id);

    // Remove existing installation
    if plugin_dir.exists() {
        let _ = fs::remove_dir_all(&plugin_dir);
    }
    fs::create_dir_all(&plugin_dir).map_err(|e| format!("Failed to create plugin dir: {}", e))?;

    // Re-open archive (consumed by by_name above)
    let file2 = fs::File::open(&src_path).map_err(|e| format!("Failed to reopen file: {}", e))?;
    let mut archive2 = zip::ZipArchive::new(file2).map_err(|e| format!("Invalid archive: {}", e))?;

    // Extract all files
    for i in 0..archive2.len() {
        let mut file = archive2.by_index(i).map_err(|e| format!("Archive error: {}", e))?;
        let outpath = plugin_dir.join(file.mangled_name());

        // Prevent path traversal
        if !outpath.starts_with(&plugin_dir) {
            continue;
        }

        if file.is_dir() {
            let _ = fs::create_dir_all(&outpath);
        } else {
            if let Some(parent) = outpath.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let mut outfile = fs::File::create(&outpath)
                .map_err(|e| format!("Failed to create file: {}", e))?;
            std::io::copy(&mut file, &mut outfile)
                .map_err(|e| format!("Failed to extract file: {}", e))?;
        }
    }

    Ok(manifest)
}

#[tauri::command]
fn uninstall_plugin(id: String) -> Result<bool, String> {
    // Strikt of een veilig oud id; nooit de pluginmap zelf of daarbuiten (#550).
    extensie_installatie::controleer_bestaand_id(&id)?;

    let plugins_dir = get_plugins_dir();
    let plugin_dir = plugins_dir.join(&id);

    // Niet tijdens een lopende installatie van een extensie.
    extensie_installatie::met_wisselslot(|| {
        if plugin_dir.exists() {
            fs::remove_dir_all(&plugin_dir).map_err(|e| format!("Failed to remove plugin: {}", e))?;
        }
        Ok(true)
    })
}

#[tauri::command]
fn read_plugin_file(plugin_id: String, file_path: String) -> Result<String, String> {
    // Validate inputs: strikt of een veilig oud id (#550)
    extensie_installatie::controleer_bestaand_id(&plugin_id)?;
    if file_path.contains("..") {
        return Err("Invalid file path".to_string());
    }

    let plugins_dir = get_plugins_dir();
    let full_path = plugins_dir.join(&plugin_id).join(&file_path);

    // Ensure the resolved path is within the plugin directory
    if !full_path.starts_with(plugins_dir.join(&plugin_id)) {
        return Err("Path traversal detected".to_string());
    }

    fs::read_to_string(&full_path).map_err(|e| format!("Failed to read file: {}", e))
}

/// Grootste bestand dat `read_plugin_asset` in één keer teruggeeft (modellen).
const PLUGIN_ASSET_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// Relatief pad binnen een extensiemap, zonder `..`/`.`-segmenten, absolute
/// paden, stationsletters of andere `:`, NUL of lege segmenten. Dezelfde regel
/// als isVeiligAssetPad in js/plugins/plugin-assets.js (#970).
fn is_veilig_asset_pad(file_path: &str) -> bool {
    !file_path.is_empty()
        && file_path.len() <= 512
        && !file_path.contains('\0')
        && !file_path.contains(':')
        && !file_path.starts_with('/')
        && !file_path.starts_with('\\')
        && file_path
            .split(|c| c == '/' || c == '\\')
            .all(|deel| !deel.is_empty() && deel != "." && deel != "..")
}

/// Een binair bestand uit de eigen map van een geïnstalleerde extensie
/// (modellen, WebAssembly; #970): `read_plugin_file` geeft alleen tekst. Zelfde
/// id-controle; het opgeloste pad (ook na symlinks) moet in de extensiemap
/// blijven.
#[tauri::command]
fn read_plugin_asset(plugin_id: String, file_path: String) -> Result<tauri::ipc::Response, String> {
    extensie_installatie::controleer_bestaand_id(&plugin_id)?;
    if !is_veilig_asset_pad(&file_path) {
        return Err("Invalid file path".to_string());
    }
    let plugin_dir = get_plugins_dir().join(&plugin_id);
    let root = fs::canonicalize(&plugin_dir).map_err(|e| format!("Extension folder not found: {}", e))?;
    let full_path = fs::canonicalize(plugin_dir.join(&file_path))
        .map_err(|e| format!("Failed to read file: {}", e))?;
    if !full_path.starts_with(&root) {
        return Err("Path traversal detected".to_string());
    }
    let meta = fs::metadata(&full_path).map_err(|e| format!("Failed to read file: {}", e))?;
    if !meta.is_file() {
        return Err("Not a file".to_string());
    }
    if meta.len() > PLUGIN_ASSET_MAX_BYTES {
        return Err(format!("Asset too large ({} bytes, max {})", meta.len(), PLUGIN_ASSET_MAX_BYTES));
    }
    let data = fs::read(&full_path).map_err(|e| format!("Failed to read file: {}", e))?;
    Ok(tauri::ipc::Response::new(data))
}

// ─── Allow FS access to a path (for testing / programmatic file opens) ────
#[tauri::command]
fn allow_fs_scope(app: tauri::AppHandle, path: String) -> Result<bool, String> {
    app.fs_scope().allow_file(&path)
        .map_err(|e| format!("{}", e))?;
    // Ook de andere schrijfwijzen (met en zonder \\?\, #1005): de scope
    // vergelijkt tekst, en een lang pad of netwerkpad komt soms in de andere
    // vorm terug.
    for variant in bestand_openen::scope_varianten(&path).into_iter().skip(1) {
        let _ = app.fs_scope().allow_file(&variant);
        if let Some(parent) = std::path::Path::new(&variant).parent() {
            let _ = app.fs_scope().allow_directory(parent, false);
        }
    }
    // Also allow the directory for related files
    if let Some(parent) = std::path::Path::new(&path).parent() {
        let _ = app.fs_scope().allow_directory(parent, false);
    }
    // De scope canonicaliseert het pad bij elke controle; lukt dat niet (soms
    // op WebDAV), dan blijft het pad erbuiten. Het document zelf leest de app
    // dan via read_document_file; hier alleen melden.
    if !app.fs_scope().is_allowed(&path) {
        eprintln!("[fs-scope] path stays outside the fs scope (cannot be canonicalized?): {}", path);
    }
    Ok(true)
}

// ─── PDF rendering via open-pdf-render (pure Rust) ────────────────────────
use open_pdf_render::{DocumentHandle, PdfRenderer};

/// Get a cached `Arc<DocumentHandle>` for the given file, or load + cache one.
///
/// First checks the DocHandleCache. On miss, falls back to the bytes cache
/// (or disk), parses the PDF once, and stores the resulting Arc in the
/// handle cache for all future commands. The handle owns its own internal
/// font registry so subsequent extract_draw_commands calls reuse all glyph
/// outlines from previous pages.
fn get_or_load_doc(
    path: &str,
    bytes_cache: &PdfBytesCache,
    handle_cache: &DocHandleCache,
) -> Result<Arc<DocumentHandle>, String> {
    // Fast path — handle already cached
    {
        let cache_map = handle_cache.0.lock().map_err(|e| format!("Handle cache lock: {}", e))?;
        if let Some(handle) = cache_map.get(path) {
            return Ok(handle.clone());
        }
    }

    // Slow path — fetch bytes (from cache or disk), parse, insert
    let bytes = {
        let mut bm = bytes_cache.0.lock().map_err(|e| format!("Bytes cache lock: {}", e))?;
        if let Some(cached) = bm.get(path) {
            cached.clone()
        } else {
            let read = fs::read(path).map_err(|e| format!("Read: {}", e))?;
            bm.insert(path.to_string(), read.clone());
            read
        }
    };

    let renderer = PdfRenderer::new();
    let handle = renderer.load_document(&bytes).map_err(|e| format!("{}", e))?;
    let arc = Arc::new(handle);

    {
        let mut hm = handle_cache.0.lock().map_err(|e| format!("Handle cache lock: {}", e))?;
        // Double-check in case another thread inserted while we were parsing.
        if let Some(existing) = hm.get(path) {
            return Ok(existing.clone());
        }
        hm.insert(path.to_string(), arc.clone());
    }
    Ok(arc)
}

/// DEBUG / BENCHMARK TOOL. Render a page using our own open-pdf-render Skia
/// kernel instead of PDFium. NOT wired into the live render path — production
/// rendering always goes through `render_pdf_page` (PDFium). Kept callable so
/// `mcp-server/skia-vs-pdfium-render.mjs` can re-run the head-to-head speed
/// comparison whenever open-pdf-render's accuracy improves.
///
/// Wire format matches `render_pdf_page` exactly: `[w:u32 LE][h:u32 LE]
/// [rgba…]`. Probes can swap one invoke for the other without unpacking
/// differently.
///
/// IMPORTANT: open-pdf-render is currently below the < 2 % pixel-diff goal
/// vs the PyMuPDF reference (per scripts/render-regression-test.py). Some
/// pages render incorrectly. Speed numbers from this command must always be
/// read with that disclaimer until the regression suite reports green.
#[tauri::command]
fn render_pdf_page_skia(
    path: String,
    page_index: u32,
    scale: f32,
    rotation: Option<i32>,
    bytes_cache: tauri::State<PdfBytesCache>,
    handle_cache: tauri::State<DocHandleCache>,
) -> Result<tauri::ipc::Response, String> {
    // Geschakelde lagen (#684): teken uit de weergavekopie.
    let path = laagweergave::tekenpad(&path);
    let extra_rot = rotation.unwrap_or(0);
    let doc = get_or_load_doc(&path, &bytes_cache, &handle_cache)?;
    let rendered = doc.render_page(page_index as usize, scale, extra_rot)
        .map_err(|e| format!("{}", e))?;
    let mut out = Vec::with_capacity(8 + rendered.rgba.len());
    out.extend_from_slice(&rendered.width.to_le_bytes());
    out.extend_from_slice(&rendered.height.to_le_bytes());
    out.extend_from_slice(&rendered.rgba);
    Ok(tauri::ipc::Response::new(out))
}

/// Whether the PDFium worker pool has finished spawning. Lets the frontend
/// skip optional work (e.g. the cold-open pre-render) that would otherwise
/// fall back to an in-process PDFium parse in the main process — on a large
/// document that fallback runs concurrently with the PDF.js parse of the
/// same bytes and can take the whole load down at startup.
#[tauri::command]
fn worker_pool_ready(
    pool: tauri::State<'_, std::sync::Arc<tokio::sync::OnceCell<worker_pool::WorkerPool>>>,
) -> bool {
    pool.get().is_some()
}

/// True when a pool error means the worker process itself went away (EOF or a
/// broken pipe), or exceeded its resource deadline. Bitmap size can still fall back.
fn worker_died(e: &anyhow::Error) -> bool {
    if worker_pool::response::is_resource_limit(e) { return true; }
    let msg = e.to_string();
    msg.ends_with(" EOF") || msg.starts_with("write to worker") || msg.starts_with("read from worker")
}

#[tauri::command]
async fn render_pdf_page(
    path: String,
    page_index: u32,
    scale: f32,
    rotation: Option<i32>,
    // Thin Lines view mode (#595): every vector stroke as a hairline.
    thin_lines: Option<bool>,
    // Page sharpen (render-sharpen.js): 0 or absent = none; see verscherpen.rs.
    sharpen: Option<f64>,
    bytes_cache: tauri::State<'_, PdfBytesCache>,
    pdfium_cache: tauri::State<'_, pdfium_renderer::PdfiumDocCache>,
    pixmap_cache: tauri::State<'_, pdfium_renderer::PixmapCacheState>,
    pool: tauri::State<'_, std::sync::Arc<tokio::sync::OnceCell<worker_pool::WorkerPool>>>,
) -> Result<tauri::ipc::Response, String> {
    // Geschakelde lagen (#684): teken uit de weergavekopie.
    let path = laagweergave::tekenpad(&path);
    let extra_rot = rotation.unwrap_or(0);
    let thin_lines = thin_lines.unwrap_or(false);
    let cache_key = pdfium_renderer::pixmap_key(&path, page_index, scale, extra_rot, thin_lines);

    // Cache fast path (unchanged)
    pixmap_cache.ensure();
    // De vergrendeling is weer vrij vóór het verscherpen (dat wacht op een
    // andere thread): alleen de kopie van het beeld gaat mee.
    let uit_cache = pixmap_cache.0.lock().ok().and_then(|guard| {
        let cached = guard.as_ref()?.get(&cache_key)?;
        let mut data = Vec::with_capacity(8 + cached.rgba.len());
        data.extend_from_slice(&cached.width.to_le_bytes());
        data.extend_from_slice(&cached.height.to_le_bytes());
        data.extend_from_slice(&cached.rgba);
        Some(data)
    });
    if let Some(data) = uit_cache {
        return Ok(verscherpen::antwoord(data, sharpen).await);
    }

    // Try the worker pool first
    if let Some(p) = pool.get() {
        match p.render(&path, page_index, scale, extra_rot, thin_lines).await {
            Ok((width, height, rgba)) => {
                let rgba_arc = std::sync::Arc::new(rgba);
                if let Ok(mut guard) = pixmap_cache.0.lock() {
                    if let Some(cache) = guard.as_mut() {
                        cache.insert(cache_key, std::sync::Arc::new(pdfium_renderer::CachedPixmap {
                            width, height, rgba: rgba_arc.clone(),
                        }));
                    }
                }
                let mut data = Vec::with_capacity(8 + rgba_arc.len());
                data.extend_from_slice(&width.to_le_bytes());
                data.extend_from_slice(&height.to_le_bytes());
                data.extend_from_slice(&rgba_arc);
                return Ok(verscherpen::antwoord(data, sharpen).await);
            }
            Err(e) => {
                // A worker can die while parsing a malformed PDF. Retrying
                // that same file inside the GUI process would move the native
                // PDFium crash into the app itself (observed with Tekst.pdf).
                // The pool already retries on a second worker; keep the
                // failure isolated and let the UI show a render error.
                // Other failures (bitmap too large for the shared
                // memory, an older worker) still fall back to in-proc.
                if worker_died(&e) {
                    return Err(format!("PDFium worker render failed: {e:#}"));
                }
                eprintln!("[render_pdf_page] pool render failed: {} — falling back to in-proc", e);
            }
        }
    }

    // Fallback: in-proc PDFium (existing path, unchanged)
    let bytes = {
        let mut bm = bytes_cache.0.lock().map_err(|e| format!("Bytes cache lock: {}", e))?;
        if let Some(cached) = bm.get(&path) {
            cached.clone()
        } else {
            let read = std::fs::read(&path).map_err(|e| format!("Read: {}", e))?;
            bm.insert(path.clone(), read.clone());
            read
        }
    };

    let handle = pdfium_renderer::get_or_load_pdfium_doc_with_bytes(
        &path,
        std::sync::Arc::new(bytes),
        &pdfium_cache,
    )?;

    let (width, height, rgba) = pdfium_renderer::render_page_to_rgba_opts(
        handle.document(),
        page_index,
        scale,
        extra_rot,
        thin_lines,
        pdfium_renderer::INPROC_SCREEN_RGBA_BYTES,
    )?;

    let rgba_arc = std::sync::Arc::new(rgba);
    if let Ok(mut guard) = pixmap_cache.0.lock() {
        if let Some(cache) = guard.as_mut() {
            cache.insert(cache_key, std::sync::Arc::new(pdfium_renderer::CachedPixmap {
                width, height, rgba: rgba_arc.clone(),
            }));
        }
    }

    let mut data = Vec::with_capacity(8 + rgba_arc.len());
    data.extend_from_slice(&width.to_le_bytes());
    data.extend_from_slice(&height.to_le_bytes());
    data.extend_from_slice(&rgba_arc);
    Ok(verscherpen::antwoord(data, sharpen).await)
}

/// Run OCR on one page. `lang` is Tesseract's `+`-joined language spec (e.g.
/// "chi_tra+eng"); defaults to "eng" if omitted. Tessdata is resolved from
/// the bundled `tessdata` resource directory — see scripts/ocr-runtime.mjs
/// and the `resources` map in tauri.conf.json.
#[tauri::command]
async fn ocr_pdf_page(
    app: tauri::AppHandle,
    path: String,
    page_index: u32,
    lang: Option<String>,
    // Extra draaiing rechtsom (0/90/180/270) vóór de herkenning, voor
    // verticale tekst (#954); de woorden staan dan in die gedraaide ruimte.
    rotation: Option<i32>,
    bytes_cache: tauri::State<'_, PdfBytesCache>,
    pdfium_cache: tauri::State<'_, pdfium_renderer::PdfiumDocCache>,
) -> Result<Vec<ocr::OcrWord>, String> {
    let bytes = {
        let mut bm = bytes_cache.0.lock().map_err(|e| format!("Bytes cache lock: {}", e))?;
        if let Some(cached) = bm.get(&path) {
            cached.clone()
        } else {
            let read = std::fs::read(&path).map_err(|e| format!("Read: {}", e))?;
            bm.insert(path.clone(), read.clone());
            read
        }
    };

    let handle = pdfium_renderer::get_or_load_pdfium_doc_with_bytes(
        &path,
        std::sync::Arc::new(bytes),
        &pdfium_cache,
    )?;

    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|e| format!("Cannot resolve resource_dir: {}", e))?;
    // Niet rechtstreeks to_str(): op Windows is resource_dir een \\?\-pad en
    // daar kan Tesseract geen "/<taal>.traineddata" achter plakken.
    let tessdata_dir = ocr::tessdata_path_for_tesseract(&resource_dir.join("tessdata"))?;

    let lang = lang.unwrap_or_else(|| "auto".to_string());

    // Tesseract inference is synchronous/blocking (and re-inits per call) —
    // run it off the async executor so it doesn't stall other IPC.
    tauri::async_runtime::spawn_blocking({
        move || ocr::ocr_page_words_rotated(handle.document(), page_index, &tessdata_dir, &lang, rotation.unwrap_or(0))
    })
    .await
    .map_err(|e| format!("OCR task panicked: {}", e))?
}


/// Pixelvergelijking van een paginapaar (#465) in de PDFium-worker: renderen
/// op de vergelijkingsresolutie, inktmaskers, automatische uitlijning en
/// clusters gebeuren in het workerproces, dus geen bitmap komt over IPC.
/// `request` is het verzoek uit js/compare/rust-compare.js (zie
/// pdfium-worker/src/compare/mod.rs, CompareRequest zonder op/id).
/// Antwoord (rauwe bytes): u32 LE lengte van de JSON-kop, de JSON-kop,
/// opvulling tot een veelvoud van 8, daarna 10 f64 per cluster.
/// Fouten beginnen met COMPARE_RUST_; de app rekent dan in JS.
/// `OPDS_RUST_COMPARE=0` dwingt de JS-weg af (ter vergelijking).
#[tauri::command]
async fn compare_pixel_pair(
    request: serde_json::Value,
    pool: tauri::State<'_, std::sync::Arc<tokio::sync::OnceCell<worker_pool::WorkerPool>>>,
) -> Result<tauri::ipc::Response, String> {
    if std::env::var("OPDS_RUST_COMPARE").ok().as_deref() == Some("0") {
        return Err("COMPARE_RUST_DISABLED".to_string());
    }
    let p = pool.get().ok_or_else(|| "COMPARE_RUST_UNAVAILABLE: worker pool not ready".to_string())?;
    let new_path = request["new"]["path"].as_str().ok_or_else(|| "COMPARE_RUST_FAILED: new.path missing".to_string())?.to_string();
    let new_page = request["new"]["page_index"].as_u64().unwrap_or(0) as u32;
    let (resp, clusters) = p
        .compare(request, &new_path, new_page)
        .await
        .map_err(|e| format!("COMPARE_RUST_UNAVAILABLE: {e:#}"))?;
    if !resp["ok"].as_bool().unwrap_or(false) {
        return Err(format!("COMPARE_RUST_FAILED: {}", resp["error"].as_str().unwrap_or("unknown")));
    }
    let head = resp.to_string().into_bytes();
    let pad = (8 - (4 + head.len()) % 8) % 8;
    let mut out = Vec::with_capacity(4 + head.len() + pad + clusters.len());
    out.extend_from_slice(&(head.len() as u32).to_le_bytes());
    out.extend_from_slice(&head);
    out.resize(out.len() + pad, 0);
    out.extend_from_slice(&clusters);
    Ok(tauri::ipc::Response::new(out))
}

/// Pixelwerk op een paginabeeld in de PDFium-worker: symbolen zoeken
/// (`symbol_scan`) en formuliervormen herkennen (`form_shapes`), zie
/// pdfium-worker/src/raster_scan/mod.rs. `request` is de aanvraag zonder op/id
/// (js/pdf/raster-scan-rust.js); het antwoord is de antwoordregel van de
/// worker. Fouten beginnen met SCAN_RUST_; een fout die de JS-functies zelf
/// ook geven (TEMPLATE_BOUNDS en dergelijke) geeft de app door, bij elke
/// andere rekent ze in JavaScript. `OPDS_RUST_SCAN=0` dwingt de JS-weg af.
#[tauri::command]
async fn raster_scan_page(
    op: String,
    request: serde_json::Value,
    pool: tauri::State<'_, std::sync::Arc<tokio::sync::OnceCell<worker_pool::WorkerPool>>>,
) -> Result<serde_json::Value, String> {
    if symbool_zoeken::uitgezet() {
        return Err("SCAN_RUST_DISABLED".to_string());
    }
    if op != "symbol_scan" && op != "form_shapes" {
        return Err(format!("SCAN_RUST_FAILED: unknown operation {op}"));
    }
    let p = pool.get().ok_or_else(|| "SCAN_RUST_UNAVAILABLE: worker pool not ready".to_string())?;
    let path = request["raster"]["path"].as_str().ok_or_else(|| "SCAN_RUST_FAILED: raster.path missing".to_string())?.to_string();
    let page = request["raster"]["page_index"].as_u64().unwrap_or(0) as u32;
    let resp = p
        .raster_scan(&op, request, &path, page)
        .await
        .map_err(|e| format!("SCAN_RUST_UNAVAILABLE: {e:#}"))?;
    if !resp["ok"].as_bool().unwrap_or(false) {
        return Err(format!("SCAN_RUST_FAILED: {}", resp["error"].as_str().unwrap_or("unknown")));
    }
    Ok(resp)
}

#[tauri::command]
async fn render_pdf_page_region(
    path: String,
    page_index: u32,
    scale: f32,
    rotation: Option<i32>,
    region_x_pt: f32,
    region_y_pt: f32,
    region_w_pt: f32,
    region_h_pt: f32,
    spread: Option<bool>,
    // Thin Lines view mode (#595), see render_pdf_page.
    thin_lines: Option<bool>,
    // Page sharpen (render-sharpen.js): 0 or absent = none; see verscherpen.rs.
    sharpen: Option<f64>,
    bytes_cache: tauri::State<'_, PdfBytesCache>,
    pdfium_cache: tauri::State<'_, pdfium_renderer::PdfiumDocCache>,
    pool: tauri::State<'_, std::sync::Arc<tokio::sync::OnceCell<worker_pool::WorkerPool>>>,
) -> Result<tauri::ipc::Response, String> {
    // Geschakelde lagen (#684): teken uit de weergavekopie.
    let path = laagweergave::tekenpad(&path);
    let extra_rot = rotation.unwrap_or(0);
    let thin_lines = thin_lines.unwrap_or(false);

    // Try the worker pool first: region tiles are small (fit the 64 MB SHM), so
    // they render in a SEPARATE process — safe for parallel/pre-cache rendering
    // and off the main thread. Falls back to in-proc PDFium on any failure
    // except a dead worker: that crash must not be repeated in the GUI process.
    if let Some(p) = pool.get() {
        match p
            .render_region(&path, page_index, scale, extra_rot, region_x_pt, region_y_pt, region_w_pt, region_h_pt, spread.unwrap_or(false), thin_lines)
            .await
        {
            Ok((width, height, rgba)) => {
                let mut data = Vec::with_capacity(8 + rgba.len());
                data.extend_from_slice(&width.to_le_bytes());
                data.extend_from_slice(&height.to_le_bytes());
                data.extend_from_slice(&rgba);
                return Ok(verscherpen::antwoord(data, sharpen).await);
            }
            Err(e) => {
                if worker_died(&e) {
                    return Err(format!("PDFium worker region render failed: {e:#}"));
                }
                eprintln!("[render_pdf_page_region] pool failed: {} — in-proc fallback", e);
            }
        }
    }

    // In-proc-fallback: gelijktijdige in-proc PDFium-renders crashen het proces
    // (double-free / heap-corruptie — de worker-pool bestaat juist daarom). Alle
    // in-proc PDFium-toegang (laden én renderen, alle drie de render-paden) wordt
    // nu centraal geserialiseerd via PDFIUM_INPROC_LOCK in pdfium_renderer, dus
    // dit pad heeft geen eigen lock meer nodig.
    let bytes = {
        let mut bm = bytes_cache.0.lock().map_err(|e| format!("Bytes cache lock: {}", e))?;
        if let Some(cached) = bm.get(&path) {
            cached.clone()
        } else {
            let read = std::fs::read(&path).map_err(|e| format!("Read: {}", e))?;
            bm.insert(path.clone(), read.clone());
            read
        }
    };

    let handle = pdfium_renderer::get_or_load_pdfium_doc_with_bytes(
        &path,
        std::sync::Arc::new(bytes),
        &pdfium_cache,
    )?;

    let (width, height, rgba) = pdfium_renderer::render_page_region_to_rgba(
        handle.document(),
        page_index,
        scale,
        extra_rot,
        region_x_pt,
        region_y_pt,
        region_w_pt,
        region_h_pt,
        thin_lines,
    )?;

    let mut data = Vec::with_capacity(8 + rgba.len());
    data.extend_from_slice(&width.to_le_bytes());
    data.extend_from_slice(&height.to_le_bytes());
    data.extend_from_slice(&rgba);
    Ok(verscherpen::antwoord(data, sharpen).await)
}

/// Snap-geometrie van een pagina uit de PDFium-worker (pdfium-worker,
/// snap_geometry.rs): eindpunten, middens en randen van de vectorinhoud in
/// de annotatieruimte die `viewport` (weergavematrix bij schaal 1, zoals de
/// app hem berekent) beschrijft. Antwoord: de `OSG1`-codering, ongewijzigd.
/// Faalt expliciet; de JavaScript-kant extraheert dan zelf met PDF.js
/// (pdf-snap-extractor.js). `OPDS_RUST_SNAP=0` dwingt die JavaScript-route af,
/// om beide te vergelijken.
#[tauri::command]
async fn extract_snap_geometry(
    path: String,
    page_index: u32,
    viewport: Vec<f64>,
    pool: tauri::State<'_, std::sync::Arc<tokio::sync::OnceCell<worker_pool::WorkerPool>>>,
) -> Result<tauri::ipc::Response, String> {
    if std::env::var("OPDS_RUST_SNAP").ok().as_deref() == Some("0") {
        return Err("PDF_SNAP_DISABLED".to_string());
    }
    let viewport: [f64; 6] = viewport
        .try_into()
        .map_err(|_| "PDF_INVALID_GEOMETRY: viewport needs 6 numbers".to_string())?;
    let Some(p) = pool.get() else {
        return Err("PDF_WORKER_UNAVAILABLE".to_string());
    };
    match p.snap_geometry(&path, page_index, viewport).await {
        Ok(bytes) => Ok(tauri::ipc::Response::new(bytes)),
        Err(e) if e.downcast_ref::<worker_pool::snap::Unsupported>().is_some() => Err("PDF_SNAP_UNSUPPORTED".to_string()),
        Err(e) => Err(format!("PDF_SNAP_FAILED: {e:#}")),
    }
}

/// Zoektekst van pagina's uit de PDFium-worker (pdfium-worker, page_text/):
/// de tekstitems zoals pdf.js ze voor het zoeken maakt, met per teken zijn
/// plek op de basislijn, voor pagina's vanaf `first` (0-gebaseerd). Antwoord:
/// de `OPT1`-codering, ongewijzigd; die noemt hoeveel pagina's erin zitten
/// (de worker stopt na een tijdsbudget). `expect_len`: de bestandslengte die
/// de app heeft; wijkt het bestand op schijf af, dan een fout. `lane`: welke
/// van de banen waarmee de app het document leest (elke baan een eigen
/// worker, zie worker_pool/text.rs). Faalt
/// expliciet; de JavaScript-kant leest de tekst dan met pdf.js
/// (js/search/rust-zoektekst.js). `OPDS_RUST_SEARCH=0` dwingt die
/// JavaScript-route af, om beide te vergelijken.
#[tauri::command]
async fn extract_search_text(
    path: String,
    first: u32,
    count: u32,
    expect_len: Option<u64>,
    lane: Option<u32>,
    pool: tauri::State<'_, std::sync::Arc<tokio::sync::OnceCell<worker_pool::WorkerPool>>>,
) -> Result<tauri::ipc::Response, String> {
    if std::env::var("OPDS_RUST_SEARCH").ok().as_deref() == Some("0") {
        return Err("PDF_TEXT_DISABLED".to_string());
    }
    let Some(p) = pool.get() else {
        return Err("PDF_WORKER_UNAVAILABLE".to_string());
    };
    match p.page_text(&path, first, count.clamp(1, 1000), expect_len, lane.unwrap_or(0).min(8)).await {
        Ok(bytes) => Ok(tauri::ipc::Response::new(bytes)),
        Err(e) if e.downcast_ref::<worker_pool::text::Unsupported>().is_some() => Err("PDF_TEXT_UNSUPPORTED".to_string()),
        Err(e) => Err(format!("PDF_TEXT_FAILED: {e:#}")),
    }
}

/// Route A: render een tegel-regio via de eigen display-list-engine i.p.v. de
/// PDFium-workers. De eerste aanroep per (pad, pagina, rotatie) bouwt de scene
/// (extract + chunk-index — seconden op een 5M-ops blad, daarna gecachet);
/// elke volgende tegel is puur parallel rasterwerk. Wire-format identiek aan
/// render_pdf_page_region: [w u32 LE][h u32 LE][rgba]. Faalt expliciet (geen
/// stille degradatie) zodat de JS-kant per pagina kan terugvallen op PDFium.
#[tauri::command]
async fn render_tile_scene_region(
    path: String,
    page_index: u32,
    rotation: Option<i32>,
    scale: f32,
    region_x_pt: f32,
    region_y_pt: f32,
    region_w_pt: f32,
    region_h_pt: f32,
    // Thin Lines view mode (#595): the scene replays every stroke as a
    // hairline. The scene itself is the same in both modes.
    thin_lines: Option<bool>,
    // Page sharpen (render-sharpen.js): 0 or absent = none; see verscherpen.rs.
    sharpen: Option<f64>,
    bytes_cache: tauri::State<'_, PdfBytesCache>,
    handle_cache: tauri::State<'_, DocHandleCache>,
    scene_cache: tauri::State<'_, TileSceneCache>,
) -> Result<tauri::ipc::Response, String> {
    // Geschakelde lagen (#684): teken uit de weergavekopie.
    let path = laagweergave::tekenpad(&path);
    let extra_rot = rotation.unwrap_or(0);
    let thin_lines = thin_lines.unwrap_or(false);
    let meta = std::fs::metadata(&path).map_err(|e| format!("stat: {}", e))?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let key = format!("{}|{}|{}|p{}|r{}", path, mtime, meta.len(), page_index, extra_rot);

    let lookup = |sc: &TileSceneCache| -> Result<Option<Arc<open_pdf_render::tile_render::TileScene>>, String> {
        let cache = sc.0.lock().map_err(|e| format!("scene-cache lock: {}", e))?;
        Ok(cache.iter().find(|(k, _)| *k == key).map(|(_, s)| s.clone()))
    };

    let scene = match lookup(&scene_cache)? {
        Some(s) => s,
        None => {
            // Build serialiseren: gelijktijdige tegel-requests voor een verse
            // pagina zouden anders elk de dure extract+index doen.
            static BUILD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
            let _b = BUILD_LOCK.lock().await;
            if let Some(s) = lookup(&scene_cache)? {
                s
            } else {
                let doc = get_or_load_doc(&path, &bytes_cache, &handle_cache)?;
                let built = tauri::async_runtime::spawn_blocking(move || -> Result<open_pdf_render::tile_render::TileScene, String> {
                    let buf = doc
                        .extract_draw_commands(page_index as usize, extra_rot)
                        .map_err(|e| format!("extract: {}", e))?;
                    let bytes = buf.into_bytes();
                    // Image-zware bladen horen op het PDFium-pad: een scene vol
                    // ge-embedde beelddata is traag én zwaar — expliciet weigeren.
                    if bytes.len() > 400 * 1024 * 1024 {
                        return Err(format!("scene te groot ({} MB)", bytes.len() / 1_048_576));
                    }
                    let buffer_mb = ((bytes.len() / 1_048_576) as u64).max(1);
                    let scene = open_pdf_render::tile_render::TileScene::build(bytes)
                        .map_err(|e| format!("scene-build: {}", e))?;
                    // Datagedreven engine-gate (corpus-benchmark 2026-07-05,
                    // 136 pagina's; herijkt 2026-07-06 na image- en clip-
                    // ondersteuning in de tegel-rasterizer): de scene is
                    // alleen sneller ÉN accuraat op lijnwerk-bladen.
                    //
                    // Image-drempel 1 MB -> 2 MB (overleg-notitie): de
                    // rasterizer tekent DrawImage-payloads nu daadwerkelijk
                    // (RGBA-raw + JPEG), en de inline-strip-emissie geeft het
                    // maatgevende strip-blad (MV-03: 4.925 strips, 1,44 MB
                    // payload, downsampled diff 7,3% -> 7,1%) net méér dan
                    // 1 MB aan correct renderende beelddata. In het corpus
                    // haalt verder geen enkel blad het JS-voorfilter
                    // (content >= 6 MB), dus deze verruiming raakt alleen de
                    // strip-klasse; de BARN-klasse (tientallen MB's JPEG,
                    // PDFium wint daar in tijd en beeld) blijft er ruim boven
                    // en wordt nog steeds geweigerd.
                    //
                    // Clip-dichtheid blijft 25/MB: clips worden nu wél
                    // gerepliceerd (tegel-maskers), maar boven deze dichtheid
                    // is het maskerpad nog niet op snelheid gemeten —
                    // versoepelen pas na een corpus-meting die dat draagt.
                    let mb = buffer_mb;
                    if scene.image_bytes > 2_000_000 {
                        return Err(format!("scene geweigerd: {} MB embedded images", scene.image_bytes / 1_048_576));
                    }
                    if scene.clip_ops / mb > 25 {
                        return Err(format!("scene geweigerd: {} clips/MB", scene.clip_ops / mb));
                    }
                    Ok(scene)
                })
                .await
                .map_err(|e| format!("join: {}", e))??;
                let arc = Arc::new(built);
                let mut cache = scene_cache.0.lock().map_err(|e| format!("scene-cache lock: {}", e))?;
                if cache.len() >= TILE_SCENE_CACHE_CAP {
                    cache.remove(0);
                }
                cache.push((key.clone(), arc.clone()));
                eprintln!("[tile-scene] gebouwd: {} chunks, cache {}/{}", arc.chunk_count(), cache.len(), TILE_SCENE_CACHE_CAP);
                arc
            }
        }
    };

    let (w, h, rgba) = tauri::async_runtime::spawn_blocking(move || {
        scene.render_region_rgba_opts(scale, region_x_pt, region_y_pt, region_w_pt, region_h_pt, thin_lines)
    })
    .await
    .map_err(|e| format!("join: {}", e))?;

    let mut data = Vec::with_capacity(8 + rgba.len());
    data.extend_from_slice(&w.to_le_bytes());
    data.extend_from_slice(&h.to_le_bytes());
    data.extend_from_slice(&rgba);
    Ok(verscherpen::antwoord(data, sharpen).await)
}

/// Goedkope zwaarte-probe: som van de GECOMPRIMEERDE content-stream-lengtes van
/// een pagina (raw stream-bytes, geen decompressie). Groot => zware vector-pagina.
/// Gebruikt door het progressieve render-pad om alleen echt-zware pagina's tegel-
/// voor-tegel te renderen.
#[tauri::command]
fn page_content_size(path: String, page_index: u32) -> Result<u64, String> {
    use lopdf::{Document, Object};
    let doc = Document::load(&path).map_err(|e| format!("load: {}", e))?;
    let pages = doc.get_pages(); // BTreeMap<u32, ObjectId>, gesorteerd op paginanummer
    let page_id = pages
        .values()
        .nth(page_index as usize)
        .copied()
        .ok_or_else(|| format!("page {} out of range", page_index))?;
    let page = doc
        .get_dictionary(page_id)
        .map_err(|e| format!("page dict: {}", e))?;
    let contents = match page.get(b"Contents") {
        Ok(c) => c,
        Err(_) => return Ok(0), // geen content-stream (bv. lege pagina) => niet zwaar
    };
    // Verzamel de stream-object-ids (Contents = Reference of Array van References).
    let mut ids: Vec<lopdf::ObjectId> = Vec::new();
    match contents {
        Object::Reference(id) => ids.push(*id),
        Object::Array(arr) => {
            for o in arr {
                if let Object::Reference(id) = o {
                    ids.push(*id);
                }
            }
        }
        _ => {}
    }
    let mut total: u64 = 0;
    for id in ids {
        if let Ok(Object::Stream(s)) = doc.get_object(id) {
            total += s.content.len() as u64;
        }
    }
    Ok(total)
}

#[tauri::command]
fn render_thumbnail(
    path: String,
    page_index: u32,
    max_width: u32,
    rotation: Option<i32>,
    skip_images: Option<bool>,
    bytes_cache: tauri::State<PdfBytesCache>,
    pdfium_cache: tauri::State<pdfium_renderer::PdfiumDocCache>,
    thumb_cache: tauri::State<ThumbnailCache>,
) -> Result<String, String> {
    // Geschakelde lagen (#684): teken uit de weergavekopie.
    let path = laagweergave::tekenpad(&path);
    let extra_rot = rotation.unwrap_or(0);
    // skip_images: PDFium renders form-data on by default but doesn't
    // expose a "drop image XObjects" knob — accept the option for API
    // compat but ignore it. PDFium is fast enough that the thumbnail
    // doesn't need to drop images.
    let _ = skip_images;

    let cache_key = (path.clone(), page_index, max_width, extra_rot);
    if let Ok(tc) = thumb_cache.0.lock() {
        if let Some(cached) = tc.get(&cache_key) {
            return Ok(cached.clone());
        }
    }

    let bytes = {
        let mut bm = bytes_cache.0.lock().map_err(|e| format!("Bytes cache lock: {}", e))?;
        if let Some(cached) = bm.get(&path) {
            cached.clone()
        } else {
            let read = std::fs::read(&path).map_err(|e| format!("Read: {}", e))?;
            bm.insert(path.clone(), read.clone());
            read
        }
    };

    let handle = pdfium_renderer::get_or_load_pdfium_doc_with_bytes(
        &path,
        std::sync::Arc::new(bytes),
        &pdfium_cache,
    )?;

    let data_url = pdfium_renderer::render_thumbnail_to_json(
        handle.document(),
        page_index,
        max_width,
        extra_rot,
    )?;

    if let Ok(mut tc) = thumb_cache.0.lock() {
        tc.insert(cache_key, data_url.clone());
    }
    Ok(data_url)
}

#[tauri::command]
fn analyze_page_type(
    path: String,
    page_index: u32,
    bytes_cache: tauri::State<PdfBytesCache>,
    handle_cache: tauri::State<DocHandleCache>,
    page_type_cache: tauri::State<PageTypeCache>,
) -> Result<String, String> {
    let key = (path.clone(), page_index);
    if let Ok(cache) = page_type_cache.0.lock() {
        if let Some(cached) = cache.get(&key) {
            return Ok(cached.clone());
        }
    }
    let doc = get_or_load_doc(&path, &bytes_cache, &handle_cache)?;
    let result = match doc.analyze_page_type(page_index as usize).map_err(|e| format!("{}", e))? {
        open_pdf_render::PageType::Vector => "vector".to_string(),
        open_pdf_render::PageType::Tile => "tile".to_string(),
    };
    if let Ok(mut cache) = page_type_cache.0.lock() {
        cache.insert(key, result.clone());
    }
    Ok(result)
}

/// Batch-classify many pages in one Tauri invoke using rayon for parallelism.
/// Used immediately after cold-open to populate `PageTypeCache` for every
/// page in the document so subsequent per-page navigation hits the cache
/// instead of paying any lopdf analyze cost. Safe to call multiple times
/// — already-cached pages are skipped in the parallel loop and the result
/// returns the cached value.
#[tauri::command]
fn analyze_page_type_batch(
    path: String,
    page_indices: Vec<u32>,
    bytes_cache: tauri::State<PdfBytesCache>,
    handle_cache: tauri::State<DocHandleCache>,
    page_type_cache: tauri::State<PageTypeCache>,
) -> Result<Vec<String>, String> {
    // Pre-fill: copy already-cached results out of the lock; remaining indices
    // get computed in parallel below. Avoids contending on the cache mutex
    // from inside the rayon workers.
    let mut results: Vec<Option<String>> = vec![None; page_indices.len()];
    let mut todo: Vec<(usize, u32)> = Vec::new();
    if let Ok(cache) = page_type_cache.0.lock() {
        for (i, &p) in page_indices.iter().enumerate() {
            if let Some(v) = cache.get(&(path.clone(), p)) {
                results[i] = Some(v.clone());
            } else {
                todo.push((i, p));
            }
        }
    }
    if todo.is_empty() {
        return Ok(results.into_iter().map(|o| o.unwrap_or_default()).collect());
    }

    let doc = get_or_load_doc(&path, &bytes_cache, &handle_cache)?;
    let pages: Vec<usize> = todo.iter().map(|&(_, p)| p as usize).collect();
    let batch_results = doc.analyze_page_types_batch(&pages);

    // Stitch new results back into `results` and into the persistent cache.
    if let Ok(mut cache) = page_type_cache.0.lock() {
        for ((i_in_input, p_idx), r) in todo.iter().zip(batch_results.iter()) {
            let s = match r {
                Ok(open_pdf_render::PageType::Vector) => "vector".to_string(),
                Ok(open_pdf_render::PageType::Tile) => "tile".to_string(),
                Err(_) => "vector".to_string(), // fall through to vector path on parse error
            };
            cache.insert((path.clone(), *p_idx), s.clone());
            results[*i_in_input] = Some(s);
        }
    }
    Ok(results.into_iter().map(|o| o.unwrap_or_default()).collect())
}

#[tauri::command]
fn extract_draw_commands(
    path: String,
    page_index: u32,
    rotation: Option<i32>,
    bytes_cache: tauri::State<PdfBytesCache>,
    handle_cache: tauri::State<DocHandleCache>,
) -> Result<tauri::ipc::Response, String> {
    // Geschakelde lagen (#684): teken uit de weergavekopie.
    let path = laagweergave::tekenpad(&path);
    let doc = get_or_load_doc(&path, &bytes_cache, &handle_cache)?;
    let extra_rot = rotation.unwrap_or(0);
    let cmds = doc.extract_draw_commands(page_index as usize, extra_rot).map_err(|e| format!("{}", e))?;
    // Raw bytes: a vector sheet yields megabytes of commands, which as
    // `Vec<u8>` would cross as a JSON array of numbers.
    Ok(tauri::ipc::Response::new(cmds.into_bytes()))
}

#[tauri::command]
fn extract_page_text(
    path: String,
    page_index: u32,
    bytes_cache: tauri::State<PdfBytesCache>,
    handle_cache: tauri::State<DocHandleCache>,
) -> Result<String, String> {
    let doc = get_or_load_doc(&path, &bytes_cache, &handle_cache)?;
    doc.extract_text_positions(page_index as usize).map_err(|e| format!("{}", e))
}

/// Invalidate the PDF bytes cache AND the parsed handle cache for a specific
/// file (call after save/modify so the next render sees the new content).
#[tauri::command]
fn invalidate_pdf_cache(
    path: String,
    bytes_cache: tauri::State<PdfBytesCache>,
    handle_cache: tauri::State<DocHandleCache>,
    thumb_cache: tauri::State<ThumbnailCache>,
    page_type_cache: tauri::State<PageTypeCache>,
) -> Result<bool, String> {
    bytes_cache.0.lock().map_err(|e| format!("Bytes cache lock: {}", e))?.remove(&path);
    handle_cache.0.lock().map_err(|e| format!("Handle cache lock: {}", e))?.remove(&path);
    if let Ok(mut tc) = thumb_cache.0.lock() {
        tc.retain(|(p, _, _, _), _| p != &path);
    }
    if let Ok(mut pc) = page_type_cache.0.lock() {
        pc.retain(|(p, _), _| p != &path);
    }
    Ok(true)
}

/// Lagen schakelen (#684): meldt de weergavekopie van een document aan
/// (`view`) of af (`None`). Vanaf nu tekenen de render-opdrachten voor `path`
/// uit die kopie. Geeft de vorige kopie terug.
#[tauri::command]
fn set_layer_view(path: String, view: Option<String>) -> Option<String> {
    laagweergave::zet(&path, view)
}

/// Alles loslaten wat het proces voor een gesloten document vasthoudt: ruwe
/// bytes, geparsede handles (lopdf én PDFium), thumbnails, paginatypen en
/// pixmaps. Alleen aanroepen als geen ander open tabblad hetzelfde pad
/// gebruikt — de JS-kant beslist dat (document-release.js). Lopende renders
/// houden hun eigen Arc vast en merken hier niets van.
#[tauri::command]
fn release_pdf_document(
    path: String,
    bytes_cache: tauri::State<PdfBytesCache>,
    handle_cache: tauri::State<DocHandleCache>,
    thumb_cache: tauri::State<ThumbnailCache>,
    page_type_cache: tauri::State<PageTypeCache>,
    pdfium_cache: tauri::State<pdfium_renderer::PdfiumDocCache>,
    pixmap_cache: tauri::State<pdfium_renderer::PixmapCacheState>,
) -> Result<bool, String> {
    laagweergave::vergeet(&path);
    bytes_cache.0.lock().map_err(|e| format!("Bytes cache lock: {}", e))?.remove(&path);
    handle_cache.0.lock().map_err(|e| format!("Handle cache lock: {}", e))?.remove(&path);
    if let Ok(mut tc) = thumb_cache.0.lock() {
        tc.retain(|(p, _, _, _), _| p != &path);
    }
    if let Ok(mut pc) = page_type_cache.0.lock() {
        pc.retain(|(p, _), _| p != &path);
    }
    let removed = pdfium_cache
        .0
        .lock()
        .map_err(|e| format!("Pdfium doc cache lock: {}", e))?
        .remove(&path);
    let had_doc = removed.is_some();
    // Closing a PDFium document waits for the global PDFium lock, which a CAD
    // export or print can hold for seconds: close it off the UI thread.
    if let Some(handle) = removed {
        std::thread::spawn(move || drop(handle));
    }
    if let Ok(mut guard) = pixmap_cache.0.lock() {
        if let Some(cache) = guard.as_mut() {
            cache.remove_path(&path);
        }
    }
    Ok(had_doc)
}

/// Clear the entire PDF bytes cache AND parsed handle cache (call on app
/// cleanup or memory pressure).
#[tauri::command]
fn clear_pdf_cache(
    bytes_cache: tauri::State<PdfBytesCache>,
    handle_cache: tauri::State<DocHandleCache>,
    thumb_cache: tauri::State<ThumbnailCache>,
    page_type_cache: tauri::State<PageTypeCache>,
    pdfium_cache: tauri::State<pdfium_renderer::PdfiumDocCache>,
    pixmap_cache: tauri::State<pdfium_renderer::PixmapCacheState>,
) -> Result<bool, String> {
    bytes_cache.0.lock().map_err(|e| format!("Bytes cache lock: {}", e))?.clear();
    handle_cache.0.lock().map_err(|e| format!("Handle cache lock: {}", e))?.clear();
    if let Ok(mut tc) = thumb_cache.0.lock() { tc.clear(); }
    if let Ok(mut ptc) = page_type_cache.0.lock() { ptc.clear(); }
    if let Ok(mut pc) = pdfium_cache.0.lock() {
        // Close the documents off the UI thread (see release_pdf_document).
        let handles: Vec<_> = pc.drain().map(|(_, handle)| handle).collect();
        if !handles.is_empty() {
            std::thread::spawn(move || drop(handles));
        }
    }
    if let Ok(mut guard) = pixmap_cache.0.lock() {
        if let Some(cache) = guard.as_mut() {
            cache.clear();
        }
    }
    Ok(true)
}

#[tauri::command]
fn get_page_dimensions(
    path: String,
    bytes_cache: tauri::State<PdfBytesCache>,
    handle_cache: tauri::State<DocHandleCache>,
) -> Result<Vec<(f32, f32)>, String> {
    let doc = get_or_load_doc(&path, &bytes_cache, &handle_cache)?;
    // Parallelized via rayon inside open-pdf-render — much faster than the
    // old sequential loop on multi-page documents.
    doc.page_dimensions_all()
        .into_iter()
        .map(|r| r.map_err(|e| format!("{}", e)))
        .collect()
}

// ─── Screenshot-annotate feature ────────────────────────────────────────────
// Read the current clipboard bitmap and return it as PNG bytes. Used by the
// "annotate clipboard screenshot" flow: Windows PrtScn (and the Snipping Tool)
// place the capture on the clipboard, and reading it in Rust via the
// clipboard-manager plugin is far more reliable than navigator.clipboard.read()
// inside WebView2. Returns an error when the clipboard holds no image.
#[tauri::command]
fn read_clipboard_image_png(app: tauri::AppHandle) -> Result<tauri::ipc::Response, String> {
    use image::ImageEncoder;
    use tauri_plugin_clipboard_manager::ClipboardExt;

    let img = app
        .clipboard()
        .read_image()
        .map_err(|e| format!("no image on clipboard: {e}"))?;
    let width = img.width();
    let height = img.height();
    let rgba = img.rgba();
    if width == 0 || height == 0 || rgba.is_empty() {
        return Err("clipboard image is empty".into());
    }

    let mut png: Vec<u8> = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("PNG encode failed: {e}"))?;
    Ok(tauri::ipc::Response::new(png))
}

// Zet een PNG (rauwe IPC-body) als bitmap op het systeemklembord. Gebruikt door
// "Kopiëren naar klembord" in het schermafbeeldingsvenster (#925): WebKitGTK
// (en WKWebView) weigeren navigator.clipboard.write() met een afbeelding zodra
// een await (het renderen op 300 dpi) de gebruikersactivatie heeft verbruikt.
// Via de clipboard-manager-plugin speelt die activatie geen rol. Het klembord
// blijft bediend zolang de app draait (de plugin houdt arboard vast).
#[tauri::command]
async fn write_clipboard_image_png(
    app: tauri::AppHandle,
    request: tauri::ipc::Request<'_>,
) -> Result<(), String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;

    let png = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes.clone(),
        tauri::ipc::InvokeBody::Json(value) => {
            serde_json::from_value::<Vec<u8>>(value.get("data").cloned().ok_or("Missing image data")?)
                .map_err(|e| e.to_string())?
        }
    };
    let rgba = tauri::async_runtime::spawn_blocking(move || {
        image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .map(|img| img.to_rgba8())
            .map_err(|e| format!("PNG decode failed: {e}"))
    })
    .await
    .map_err(|e| e.to_string())??;
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return Err("image is empty".into());
    }
    app.clipboard()
        .write_image(&tauri::image::Image::new(rgba.as_raw(), width, height))
        .map_err(|e| format!("clipboard write failed: {e}"))
}

// Register/unregister the PrtScn global hotkey based on the user preference.
// Called from the frontend when the "intercept PrtScn" preference changes and
// at startup. Idempotent. Desktop only — the global-shortcut plugin has no
// mobile backend, so on Android this is a no-op stub (see below).
#[tauri::command]
fn set_prtscn_hotkey(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut};

    // Een headless testinstantie neemt de PrtScn-toets van de gebruiker niet over.
    if enabled && headless::actief() {
        headless::meld("PrtScn hotkey registration");
        return Ok(());
    }

    let shortcut = Shortcut::new(None, Code::PrintScreen);
    let gs = app.global_shortcut();
    let is_registered = gs.is_registered(shortcut);
    if enabled && !is_registered {
        gs.register(shortcut).map_err(|e| e.to_string())?;
    } else if !enabled && is_registered {
        gs.unregister(shortcut).map_err(|e| e.to_string())?;
    }
    Ok(())
}


/// Mobile entry point: tauri vereist een argumentloze functie; de desktop-
/// varianten geven StartupOpts (mcp-server/poort) door via main.rs.
#[cfg(mobile)]
#[tauri::mobile_entry_point]
fn run_mobile() {
    run(StartupOpts::default());
}

pub fn run(opts: StartupOpts) {
    // Normaal al gedaan in main(); eenmalig, dus hier alleen een vangnet.
    draagbaar::activeer();
    eprintln!(
        "[startup] mcp_server={}, mcp_port={}",
        opts.mcp_server, opts.mcp_port
    );
    if beheerd::is_beheerd() {
        eprintln!("[startup] managed mode ({}=1): first-run prompts off", beheerd::OMGEVINGSVARIABELE);
    }

    // Capture the MCP options for `setup()` — we now spawn the MCP server
    // *after* Tauri has produced an `AppHandle` so the new `app_*` tools
    // can emit events into the live WebView. Previously the server was
    // started here (before the builder ran) and never had a handle.
    let mcp_enabled = opts.mcp_server;
    let mcp_port = opts.mcp_port;

    let args: Vec<String> = std::env::args().collect();

    // Handle --version / --help before Tauri::Builder::default().run() so these
    // flags print and exit instead of hanging in the event loop. Running the
    // full builder also squats the single-instance DBus name, which blocks
    // subsequent normal launches until the name is released.
    for arg in args.iter().skip(1) {
        match arg.as_str() {
            "--version" | "-V" => {
                println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--help" | "-h" => {
                println!("Usage: {} [OPTIONS] [FILE...]", env!("CARGO_PKG_NAME"));
                println!();
                println!("Options:");
                println!("  -h, --help       Print this help and exit");
                println!("  -V, --version    Print version and exit");
                println!();
                println!("  --combine FILE...         Combine the files into one new PDF (File Explorer action)");
                println!("  --convert-combine FILE... Same as --combine; images are converted first");
                println!("  --convert-to-pdf FILE...  Convert each image to a PDF next to it (alias --convert)");
                println!();
                println!("Files:");
                println!("  One or more .pdf files to open.");
                std::process::exit(0);
            }
            _ => {}
        }
    }

    // Crashlog en noodmomentopname bij een paniek (crashlog.rs, herstel.rs):
    // zo vroeg mogelijk, zodat ook een paniek tijdens het opstarten gelogd wordt.
    crashlog::installeer_paniekhaak();
    let tweede_instantie = instantie::is_nieuwe_instantie(&args);

    // Headless testinstantie (OPDS_HEADLESS=1, zie headless.rs): vaststellen en
    // de WebView2-argumenten zetten vóór er een draad of venster bestaat.
    let headless_browser_args = if headless::init(mcp_enabled) {
        let browser_args = headless::bereid_omgeving_voor();
        eprintln!("[startup] headless: window off-screen, no focus; WebView2 args: {browser_args}");
        Some(browser_args)
    } else {
        None
    };

    // Collect any PDF file paths passed on the command line (for file associations).
    // A relative path from `AppRun <path>` is resolved against the directory
    // the user started from: AppRun itself changes to $APPDIR/usr (#863).
    let startmap = openaec_omgeving::startmap_voor_argumenten(
        openaec_omgeving::huidige(),
        &openaec_omgeving::Waarneming::van_systeem(),
    );
    let opened_files: Vec<String> = args
        .iter()
        .skip(1)
        .filter_map(|arg| pdf_uit_argument(arg))
        .map(|pad| openaec_omgeving::absoluut_argument(&pad, startmap.as_deref()))
        .collect();
    if !opened_files.is_empty() {
        eprintln!("[startup] command-line files: {opened_files:?}");
    }

    // Spawn the PDFium worker pool. Failures here are non-fatal — the
    // existing in-proc PDFium path serves as fallback when the pool is
    // unavailable.
    let pool: Arc<tokio::sync::OnceCell<worker_pool::WorkerPool>> = Arc::new(tokio::sync::OnceCell::new());
    let pool_for_init = pool.clone();
    // Shared-memory files (64 MB per worker) of earlier app processes that
    // crashed or were killed stay behind in the temp dir; remove them in the
    // background. Files of running instances are left alone.
    std::thread::spawn(worker_pool::sweep_stale_shm_files);
    tauri::async_runtime::spawn(async move {
        // De pdfium-worker-sidecar staat na bundling naast de hoofdbinary. De
        // bestandsnaam is platform-afhankelijk (`pdfium-worker.exe` op Windows,
        // `pdfium-worker` zonder extensie op macOS/Linux); worker_exe_path()
        // levert het juiste pad zodat de pool ook op macOS/Linux gevonden wordt.
        let worker_exe = worker_pool::worker_exe_path();

        if !worker_exe.exists() {
            eprintln!("[pool] pdfium-worker sidecar not found at {:?} — pool disabled, using in-proc PDFium", worker_exe);
            return;
        }

        match worker_pool::spawn::spawn_pool(4, &worker_exe).await {
            Ok(workers) => {
                let pool = worker_pool::WorkerPool::new(workers);
                if pool.is_ready() {
                    eprintln!("[pool] initialised with {} workers", pool.workers.len());
                    let _ = pool_for_init.set(pool);
                    // Idle-trim: open pagina-handles in de workers kosten op
                    // zware CAD-pagina's ruim 1 GB per worker. Na 5 min zonder
                    // renders geven de workers die parse-state terug; de
                    // eerstvolgende render betaalt eenmalig de her-parse.
                    // 5 min (niet korter): tijdens actief werken moet de
                    // handle heet blijven, anders voelt elke zoom na een
                    // denkpauze weer traag. Eigen OS-thread (geen
                    // tokio::time-afhankelijkheid).
                    let pool_for_trim = pool_for_init.clone();
                    std::thread::spawn(move || loop {
                        std::thread::sleep(std::time::Duration::from_secs(60));
                        if let Some(p) = pool_for_trim.get() {
                            tauri::async_runtime::block_on(p.trim_if_idle(300_000));
                        }
                    });
                } else {
                    eprintln!("[pool] no workers became ready — pool disabled");
                    pool.shutdown().await;
                }
            }
            Err(e) => {
                eprintln!("[pool] spawn_pool failed: {} — pool disabled", e);
            }
        }
    });

    let mut builder = tauri::Builder::default()
        .manage(OpenedFiles(Mutex::new(opened_files)))
        .manage(central_review_transport::Transport::default())
        .manage(LockedFiles(Mutex::new(HashMap::new())))
        .manage(spraak::SpraakProces::default())
        .manage(beveiligde_pdf::BeveiligdeKopieen::default())
        .manage(pdf_beveiliging::BeveiligingsSessies::default())
        .manage(PdfBytesCache(Mutex::new(HashMap::new())))
        .manage(DocHandleCache(Mutex::new(HashMap::new())))
        .manage(TileSceneCache(Mutex::new(Vec::new())))
        .manage(ThumbnailCache(Mutex::new(HashMap::new())))
        .manage(PageTypeCache(Mutex::new(HashMap::new())))
        .manage(pdfium_renderer::PdfiumDocCache::default())
        .manage(pdfium_renderer::PixmapCacheState::default())
        .manage(cad_export::CadExportJobs::default())
        .manage(cad_import::CadImportState::default())
        .manage(batch_ocr::BatchOcrDoc::default())
        .manage(scanner::ScanJobs::default())
        .manage(pool.clone())
        .manage(mcp_app_bridge::McpAppBridge::new())
        .manage(mcp_extensie_tools::ExtensieTools::default())
        .plugin(tauri_plugin_fs::init());
    // Headless: dezelfde plugin-naam en commando's, maar zonder native vensters.
    builder = if headless::actief() {
        builder.plugin(headless::dialoog_plugin())
    } else {
        builder.plugin(tauri_plugin_dialog::init())
    };
    builder = builder
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_store::Builder::default().build());
    // Uitvallende WebView2-processen in het crashlog (crashlog.rs); alleen op
    // het bureaublad (op Windows via WebView2, elders een lege plugin).
    #[cfg(desktop)]
    {
        builder = builder.plugin(crashlog::plugin());
    }
    // Linux: 2D-canvasversnelling uit in elke WebView (#924, crash B in
    // Skia's GPU-pad); OPDS_CANVAS_GPU=1 laat haar aan.
    {
        builder = builder.plugin(linux_runtime::webview_plugin());
    }
    // Remember the main window's size, position and maximised state between
    // sessions (#1025). Only the first instance saves and restores it: a
    // torn-off tab is a second instance with its own "main" window, and a
    // headless test instance keeps its off-screen window. Visibility and
    // decorations are left to the app.
    #[cfg(desktop)]
    if !tweede_instantie && !headless::actief() {
        use tauri_plugin_window_state::StateFlags;
        builder = builder.plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED)
                .build(),
        );
    }
    // Drag-out window detach is desktop-only (no mobile backend in the crate).
    {
        builder = builder.plugin(tauri_plugin_drag::init());
    }

    // Single-instance and updater plugins are desktop-only.
    //
    // A torn-off tab opens in a SECOND app instance: its own process, started
    // with `--new-instance` (see instantie.rs and
    // window_mgmt::spawn_window_with_pdf). Such a process must NOT register
    // single-instance — otherwise it would forward its PDF arg to the primary
    // instance and exit. `OPDS_DETACHED=1` also skips it (test instances next
    // to the installed app); add `OPDS_HEADLESS=1` to keep such a test
    // instance off-screen and out of focus (headless.rs).
    {
        // Linux only (the plugin uses D-Bus there): without a usable session
        // bus address zbus would panic inside the plugin; without any bus the
        // plugin just runs every start on its own.
        let skip_single_instance = instantie::slaat_single_instance_over(
            &args,
            std::env::var("OPDS_DETACHED").ok().as_deref(),
        ) || !openaec_omgeving::sessiebus_bruikbaar(
            std::env::var("DBUS_SESSION_BUS_ADDRESS").ok().as_deref(),
        );
        if !skip_single_instance {
            builder = builder
                .plugin(tauri_plugin_single_instance::init(|app: &tauri::AppHandle, argv: Vec<String>, _cwd: String| {
                    if headless::actief() {
                        headless::meld("single-instance focus");
                    } else if let Some(window) = app.get_webview_window("main") {
                        let _ = window.unminimize();
                        let _ = window.set_focus();
                    }
                    let files: Vec<String> = argv.iter()
                        .filter_map(|arg| pdf_uit_argument(arg))
                        .collect();
                    for path in &files {
                        let _ = app.fs_scope().allow_file(path);
                    }
                    if !files.is_empty() {
                        let _ = app.emit("open-files", &files);
                    }
                }));
        }
        // Managed mode (OPENAEC_BEHEERD=1): the SuperCloud refreshes the image,
        // so no updater plugin at all: no check, no prompt, no download. A Flatpak
        // gets its updates from Flathub (#389), so no built-in updater there either.
        if !beheerd::is_beheerd() {
            if !linux_runtime::running_in_flatpak() {
                builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
            }
        } else {
            eprintln!("[openaec] managed mode: updater disabled");
        }
        // The app never starts the OpenAEC Installer (no `openaec-update` plugin):
        // at every start the plugin let the installer check in the background, and
        // an installer that cannot do that opened its window instead. Updates go
        // through the updater above; the installer updates apps when the user
        // opens it.
    }
    // window.__OPENAEC__ for the frontend: managed mode and the work folder.
    builder = builder.plugin(openaec_omgeving::plugin());

    // Global system hotkey (PrtScn) for the opt-in "screenshot annotate"
    // feature. The shortcut is NOT registered here — the plugin only installs
    // the dispatch handler. The frontend calls `set_prtscn_hotkey(true)` when
    // the user enables the preference, which actually registers PrtScn. On
    // press we bring the main window to the foreground and notify the WebView,
    // which reads the clipboard image onto a fresh annotate canvas. Desktop
    // only (the crate has no mobile backend).
    {
        use tauri_plugin_global_shortcut::ShortcutState;
        builder = builder.plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        if headless::actief() {
                            headless::meld("PrtScn show/focus");
                        } else if let Some(window) = app.get_webview_window("main") {
                            let _ = window.unminimize();
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                        let _ = app.emit("prtscn-screenshot", ());
                    }
                })
                .build(),
        );
    }

    // Headless: vensters buiten beeld parkeren en zonder activeren tonen.
    if headless::actief() {
        builder = builder.plugin(headless::plugin());
    }

    let mut context = tauri::generate_context!();
    // Draagbare modus (#881): eigen single-instance-sleutel per map.
    draagbaar::pas_context_aan(&mut context);
    #[cfg(desktop)]
    startup_window::prepare(&mut context);
    if let Some(browser_args) = &headless_browser_args {
        headless::pas_context_aan(&mut context, browser_args);
    }

    builder
        .setup(move |app| {
            // Mobiel (#596): voorkeuren, sessie en catalogi in de schrijfbare
            // app-datamap, vóór er een commando draait dat ze leest.
            #[cfg(mobile)]
            match app.path().app_local_data_dir() {
                Ok(map) => datamap::zet_platform_map(map),
                Err(e) => eprintln!("[startup] geen app-datamap: {e}"),
            }

            // Als eerste, zodat alles wat hierna met `log::` gemeld wordt ook
            // ergens aankomt. Zonder logger zijn die macro's lege hulzen.
            // Mislukt het opzetten, dan draait de app gewoon door.
            match logboek::registreer(app.handle()) {
                Ok(map) => eprintln!("[startup] logboek in {}", map.display()),
                Err(e) => eprintln!("[startup] geen logboek: {e}"),
            }

            // Herstel van niet-opgeslagen werk (herstel.rs): de paniekhaak
            // kan nu de frontend bereiken, en de markering "deze instantie
            // draait" gaat pas weg bij een nette afsluiting.
            crashlog::koppel(app.handle());
            herstel::start(tweede_instantie);
            // SIGTERM (logout in the SuperCloud, `docker stop`): recovery
            // files for unsaved work, then a normal exit.
            // Only in a container or managed mode; a normal desktop keeps the
            // default SIGTERM behaviour.
            {
                if openaec_omgeving::huidige().sigterm_afhandelen() {
                    openaec_omgeving::installeer_sigterm(app.handle().clone());
                }
            }

            let diagnostics_path = datamap::tauri_map(app.handle(), datamap::TauriMap::Logs)
                .map_err(|e| format!("Cannot resolve app log directory: {e}"))?
                .join("startup-diagnostics.jsonl");
            let diagnostics = startup_diagnostics::StartupDiagnostics::new(diagnostics_path);
            diagnostics.record("native-created", None);
            app.manage(diagnostics);
            #[cfg(desktop)]
            startup_window::start_watchdog(app.handle());

            // Ondertekende versies (pdf_signed_revision) ouder dan zeven dagen
            // opruimen; vangnet naast het opruimen door de UI. Ruim genoeg, zodat
            // een tabblad dat in een andere, nog draaiende instantie open staat
            // zijn bestand niet kwijtraakt. Fouten negeren.
            if let Ok(cachemap) = datamap::tauri_map(app.handle(), datamap::TauriMap::Cache) {
                std::thread::spawn(move || {
                    handtekening::verifieer::ruim_ondertekende_versies_op(
                        &handtekening::verifieer::map_ondertekende_versies(&cachemap),
                        handtekening::verifieer::OPRUIMEN_NA,
                    );
                });
            }
            // Voorbeeld-PDF's van het importvenster voor DWG en DXF die van een
            // vorige keer zijn blijven staan (#400). De import kiest zijn map
            // zelf, op een plek (cad_import::voorbeeldmap).
            if let Ok(voorbeelden) = cad_import::voorbeeldmap(app.handle()) {
                std::thread::spawn(move || {
                    cad_import::sweep_previews(&voorbeelden, cad_import::PREVIEW_MAX_AGE);
                });
            }

            // Ontsleutelde werkkopieën van PDF's met een wachtwoord die van een
            // vorige keer zijn blijven staan (beveiligde_pdf.rs). Kopieën van
            // een andere, nog draaiende instantie blijven staan.
            std::thread::spawn(|| {
                beveiligde_pdf::ruim_oude_kopieen_op(&beveiligde_pdf::werkmap());
            });

            // Grant FS plugin scope for command-line files (file association)
            for path in app.state::<OpenedFiles>().0.lock().unwrap().iter() {
                let _ = app.fs_scope().allow_file(path);
            }

            // Set the window icon (desktop only — not applicable on Android)
            if let Some(window) = app.get_webview_window("main") {
                let icon_bytes = include_bytes!("../icons/icon.png");
                if let Ok(icon) = tauri::image::Image::from_bytes(icon_bytes) {
                    let _ = window.set_icon(icon);
                }
            }

            // Spawn the in-process MCP server (deferred until here so the
            // `app_*` tools can dispatch events into the live WebView via
            // the AppHandle stored in the server's AppState). Refuses to
            // start in release builds unless OPS_ENABLE_MCP=1 (handled
            // inside `mcp_server::start`).
            // OpenAEC Linux: MCP server on OPENAEC_MCP_POORT, only with the
            // secret from OPENAEC_MCP_GEHEIM and never for a browser (#863).
            // A second instance (torn-off tab) inherits the environment but
            // leaves the one port to the main instance.
            let beheerde_mcp = openaec_omgeving::huidige().mcp.clone().filter(|_| !tweede_instantie);
            if let Some(instelling) = beheerde_mcp {
                if mcp_enabled {
                    eprintln!("[openaec] --mcp-server ignored: the MCP server comes from OPENAEC_MCP_POORT");
                }
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = mcp_server::start_beheerd(instelling, Some(app_handle)).await {
                        eprintln!("[mcp] managed server failed: {e}");
                    }
                });
            } else if mcp_enabled {
                let app_handle = app.handle().clone();
                let test_pdfs_dir = mcp_server::resolve_test_pdfs_dir(
                    std::env::var_os("OPS_TEST_PDFS_DIR").map(std::path::PathBuf::from),
                );
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = mcp_server::start(mcp_port, test_pdfs_dir, Some(app_handle)).await {
                        eprintln!("[mcp] server failed: {e}");
                    }
                });
            }

            // PDFium initialisation — must run before any Tauri command.
            // resource_dir() returns the directory where Tauri's bundle.resources
            // land. In dev (cargo run) that's target/debug/; in release it's
            // the installer-payload root.
            let resource_dir = app
                .path()
                .resource_dir()
                .map_err(|e| format!("Cannot resolve resource_dir: {}", e))?;

            // Non-fatal: if PDFium can't load (e.g. libpdfium.so missing from a
            // Linux bundle), the app still starts and the UI loads — rendering
            // reports an error instead of aborting at startup.
            match pdfium_renderer::init_pdfium(&resource_dir) {
                Ok(()) => log::info!("PDFium initialised from {:?}", resource_dir),
                Err(e) => log::error!("PDFium initialisation failed (rendering disabled): {}", e),
            }


            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_installed_font_families,
            systeem_lettertypen::systeem_lettertype_zoeken,
            systeem_lettertypen::systeem_lettertype_lezen,
            pdf_beveiliging::pdf_security_new,
            pdf_beveiliging::pdf_security_permissions,
            pdf_beveiliging::pdf_security_unlock_owner,
            pdf_beveiliging::pdf_security_release,
            pdf_beveiliging::write_protected_pdf_atomic,
            spraak::tts_info,
            spraak::tts_speak,
            spraak::tts_stop,
            get_opened_file,
            updates_managed_externally,
            save_session,
            load_session,
            get_username,
            write_file,
            write_file_atomic,
            cancel_file_write,
            write_file_new,
            central_review::central_review_login,
            central_review::central_review_json,
            central_review::central_review_upload,
            central_review::central_review_download,
            central_review::central_review_logout,
            shared_review::shared_review_list,
            shared_review::shared_review_read,
            shared_review::shared_review_write,
            write_downloaded_file_new,
            file_exists,
            open_url,
            is_dev_mode,
            lock_file,
            unlock_file,
            read_document_file,
            lock_document_file,
            is_default_pdf_app,
            default_pdf_app_status,
            open_default_apps_settings,
            get_printers,
            print_pdf,
            open_printer_properties,
            printer_papier,
            printer_bedrukbaar,
            voorbeeld_tekening_pad,
            get_temp_dir,
            write_temp_pdf,
            delete_file,
            rename_file,
            open_pdf_in_default_viewer,
            reveal_in_file_manager,
            download_pdf_from_url,
            list_pdf_files,
            list_pdf_file_stats,
            save_preferences,
            load_preferences,
            zet_vloeiend_scrollen,
            save_catalog,
            load_catalog,
            delete_catalog,
            play_alert_sound,
            list_plugins,
            install_plugin,
            uninstall_plugin,
            read_plugin_file,
            read_plugin_asset,
            // Extensies installeren en herladen zonder dialoog (#550)
            extensie_installatie::extensie_lijst,
            extensie_installatie::extensie_inspecteer,
            extensie_installatie::extensie_installeer,
            extensie_installatie::extensie_bevestig,
            extensie_installatie::extensie_terugdraaien,
            // Extensiecatalogus: downloads via https (#969)
            extensie_catalogus::extensie_tekst_ophalen,
            extensie_catalogus::extensie_pakket_ophalen,
            extensie_catalogus::extensie_pakket_opruimen,
            render_pdf_page,
            ocr_pdf_page,
            worker_pool_ready,
            render_pdf_page_region,
            compare_pixel_pair,
            verscherpen::sharpen_rust_beschikbaar,
            raster_scan_page,
            symbool_zoeken::symbol_match_raster,
            render_tile_scene_region,
            extract_snap_geometry,
            extract_search_text,
            page_content_size,
            get_page_dimensions,
            invalidate_pdf_cache,
            handtekening::pdf_certificate_info,
            handtekening::pdf_signature_list,
            handtekening::pdf_signed_revision,
            handtekening::pdf_sign,
            release_pdf_document,
            set_layer_view,
            clear_pdf_cache,
            analyze_page_type,
            analyze_page_type_batch,
            render_pdf_page_skia,
            extract_draw_commands,
            extract_page_text,
            batch_ocr::batch_ocr_list,
            batch_ocr::batch_ocr_allow_output,
            batch_ocr::batch_ocr_inspect,
            batch_ocr::batch_ocr_page,
            batch_ocr::batch_ocr_release,
            scanner::scanner_backend,
            scanner::scanner_list,
            scanner::scanner_acquire,
            scanner::scanner_cancel,
            scanner::scanner_read_page,
            scanner::scanner_discard,
            render_thumbnail,
            render_to_png::render_page_to_png,
            cad_export::export_page_to_cad,
            cad_export::cancel_cad_export,
            cad_export::scan_page_for_cad,
            pdfx_cmyk::pdfx_list_cmyk_profiles,
            pdfx_cmyk::pdfx_inspect_profile,
            pdfx_cmyk::pdfx_convert_to_cmyk,
            cad_import::scan_cad_file,
            cad_import::import_cad_to_pdf,
            cad_import::cancel_cad_import,
            cad_import::release_cad_import,
            cad_import::locate_cad_externals,
            cad_import::check_cad_search_paths,
            cad_import::preview_cad_import,
            cad_import::discard_cad_preview,
            cad_import::cad_import_limits,
            cad_import::cad_import_dir,
            allow_fs_scope,
            beveiligde_pdf::open_protected_pdf,
            beveiligde_pdf::release_protected_pdf,
            beveiligde_pdf::verify_pdf_owner_password,
            mcp_app_bridge::app_response,
            mcp_app_bridge::mcp_bridge_ready,
            mcp_extensie_tools::mcp_set_extension_tools,
            mcp_koppeling::mcp_instellen,
            mcp_koppeling::mcp_status,
            accounts::accounts_sign_in,
            accounts::accounts_get_user,
            accounts::accounts_sign_out,
            accounts::accounts_fetch,
            accounts::accounts_upload_file,
            accounts::accounts_download_file,
            accounts::accounts_brand_logo,
            email::email_pdf,
            email::verzendkopie_schrijven,
            window_mgmt::spawn_window_with_pdf,
            window_mgmt::try_dock_pdf_at_screen,
            window_mgmt::close_window_by_label,
            window_mgmt::current_window_label,
            window_mgmt::exit_detached_process,
            window_mgmt::drag_icon_path,
            window_mgmt::tab_drag_preflight,
            window_mgmt::detach_diag,
            instantie::instance_role,
            beheerd::managed_mode,
            instantie::tab_drop_target,
            startup_window::startup_window_ready,
            startup_window::startup_font_deadline,
            startup_diagnostics::startup_diagnostic,
            startup_diagnostics::startup_diagnostics_path,
            datamap::app_data_dir_effectief,
            draagbaar::portable_status,
            herstel::herstel_info,
            herstel::herstel_schrijf,
            monochroom::monochroom_rust_beschikbaar,
            monochroom::monochroom_beeld,
            herstel::herstel_lees,
            herstel::herstel_lees_tekst,
            herstel::herstel_lijst,
            herstel::herstel_verwijder,
            herstel::herstel_hernoem,
            herstel::herstel_processen,
            herstel::herstel_claim,
            herstel::herstel_werkkopie,
            herstel::herstel_kopieer,
            herstel::herstel_bron_info,
            herstel::herstel_crashlog,
            herstel::herstel_log,
            herstel::herstel_schoon_afsluiten,
            herstel::herstel_nood_klaar,
            herstel::herstel_test_paniek,
            read_clipboard_image_png,
            write_clipboard_image_png,
            set_prtscn_hotkey,
        ])
        .build(context)
        .expect("error while building tauri application")
        .run(|_app, _event| {
            // Bij afsluiten de ontsleutelde werkkopieën van deze instantie
            // opruimen (best effort; wat blijft staan ruimt de volgende start op).
            if let tauri::RunEvent::Exit = &_event {
                beveiligde_pdf::ruim_eigen_kopieen_op(&_app.state::<beveiligde_pdf::BeveiligdeKopieen>());
                // De markering "draait" alleen weg na een nette afsluiting van
                // de frontend; anders ziet de volgende start een crash.
                herstel::bij_afsluiten();
                // Stop de PDFium-workers en verwijder hun SHM-bestanden
                // (4 x 64 MB in de temp-map); anders blijven die na elke
                // start staan. Als laatste, zodat een fout hier de twee
                // stappen hierboven nooit overslaat.
                if let Some(pool) = _app.try_state::<Arc<tokio::sync::OnceCell<worker_pool::WorkerPool>>>() {
                    worker_pool::shutdown_on_exit(&pool);
                }
            }
        });
}

#[cfg(test)]
mod geopend_document_tests {
    use super::{geopend_pdf_document, pdf_uit_argument};

    fn url(s: &str) -> url::Url {
        url::Url::parse(s).expect("geldige URL")
    }

    #[test]
    fn android_content_uri_opent_ook_zonder_pdf_extensie() {
        // "Openen met" uit een bestandsbeheerder (#593): opaak document-id.
        let uri = "content://com.android.providers.downloads.documents/document/msf%3A1234";
        assert_eq!(geopend_pdf_document(&url(uri), true).as_deref(), Some(uri));
        let uri = "content://media/external/file/42";
        assert_eq!(geopend_pdf_document(&url(uri), true).as_deref(), Some(uri));
    }

    #[test]
    fn content_uri_telt_alleen_op_android() {
        let uri = url("content://com.example.provider/plan.pdf");
        assert_eq!(geopend_pdf_document(&uri, false), None);
    }

    #[test]
    fn bestands_url_wordt_een_pad_en_alleen_voor_pdf() {
        let pad = std::env::temp_dir().join("Plan.PDF");
        let bestand = url::Url::from_file_path(&pad).expect("absoluut pad");
        for android in [false, true] {
            assert_eq!(
                geopend_pdf_document(&bestand, android),
                Some(pad.to_string_lossy().to_string())
            );
        }
        let tekst = url::Url::from_file_path(std::env::temp_dir().join("notities.txt")).unwrap();
        assert_eq!(geopend_pdf_document(&tekst, true), None);
    }

    #[test]
    fn argument_pad_of_file_uri_wordt_een_pdf_pad() {
        // Gewoon pad (Windows, %F-starters, terminal).
        assert_eq!(pdf_uit_argument("/home/a/Plan.PDF").as_deref(), Some("/home/a/Plan.PDF"));
        assert_eq!(pdf_uit_argument("C:\\tekening.pdf").as_deref(), Some("C:\\tekening.pdf"));
        // %U-starter (#748): file-URI met procentcodering.
        let pad = std::env::temp_dir().join("Bouw tekening #2.pdf");
        let uri = url::Url::from_file_path(&pad).expect("absoluut pad");
        assert!(uri.as_str().contains("%20"), "{uri}");
        assert_eq!(pdf_uit_argument(uri.as_str()), Some(pad.to_string_lossy().to_string()));
        assert_eq!(pdf_uit_argument(&uri.as_str().replacen("file", "FILE", 1)), Some(pad.to_string_lossy().to_string()));
    }

    #[test]
    fn argument_met_niet_ascii_tekens_raakt_niet_in_paniek() {
        // Byte 7 valt midden in een UTF-8-teken: geen paniek bij de start.
        for a in ["ééééé.pdf", "/tmp/€€€.pdf", "C:\\Bé€sd.pdf", "Ünterlagen/Plan.pdf"] {
            assert_eq!(pdf_uit_argument(a).as_deref(), Some(a), "{a}");
        }
        assert_eq!(pdf_uit_argument("ééééé.txt"), None);
        // Een file-URI van een pad met niet-ASCII-tekens wordt dat pad.
        let pad = std::env::temp_dir().join("Bouwtekening é€ ü.pdf");
        let uri = url::Url::from_file_path(&pad).expect("absoluut pad");
        assert_eq!(pdf_uit_argument(uri.as_str()), Some(pad.to_string_lossy().to_string()));
    }

    #[test]
    fn argument_zonder_pdf_opties_en_andere_schemas_tellen_niet() {
        for a in ["--version", "-psn_0_123.pdf", "notities.txt", "file:///tmp/notities.txt",
                  "openpdfstudio://open/x.pdf", "https://example.com/a.pdf", "file://"] {
            assert_eq!(pdf_uit_argument(a), None, "{a}");
        }
    }

    #[test]
    fn deep_links_en_web_urls_blijven_buiten_beschouwing() {
        for s in ["openpdfstudio://open/x.pdf", "https://open/x.pdf", "http://example.com/a.pdf"] {
            assert_eq!(geopend_pdf_document(&url(s), true), None, "{s}");
        }
    }
}

