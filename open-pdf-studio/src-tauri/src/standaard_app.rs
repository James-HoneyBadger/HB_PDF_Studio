//! Of HB PDF Studio de standaard-app voor .pdf is: de desktop-id die
//! `xdg-mime query default application/pdf` noemt, en wat de app kan als die
//! vraag niet te stellen is (Flatpak, Snap, geen xdg-utils).
//! `StandaardAppStatus` zegt het de JS-kant, die dan uitleg toont in plaats
//! van stil te falen.

use serde::Serialize;

/// The .desktop ids this app is installed under: the .deb/.rpm/AppImage
/// bundle (productName), the Snap and older builds, and the Flatpak app id.
pub const EIGEN_DESKTOP_IDS: &[&str] = &[
    "HB PDF Studio.desktop",
    "Open PDF Studio.desktop",
    "open-pdf-studio.desktop",
    "open-pdf-studio_open-pdf-studio.desktop",
    "org.openaec.OpenPdfStudio.desktop",
];

/// Is `desktop_id` (the output of `xdg-mime query default`) this app?
pub fn is_onze_desktop_id(desktop_id: &str) -> bool {
    let id = desktop_id.trim();
    EIGEN_DESKTOP_IDS.iter().any(|e| e.eq_ignore_ascii_case(id))
}

/// What the app knows and can do about the default PDF app.
/// `is_default`: None = cannot be checked here. `reason`: why it cannot be
/// checked or set ("flatpak", "snap", "xdg-mime-missing"), else None.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StandaardAppStatus {
    pub is_default: Option<bool>,
    pub current: Option<String>,
    pub can_set: bool,
    pub reason: Option<&'static str>,
}

/// Managed mode (`OPENAEC_BEHEERD=1`): the environment arranges the file
/// association. Counts as default, so no bar at startup, and is not looked up
/// in the registry or with xdg-mime.
pub fn beheerd_status() -> StandaardAppStatus {
    StandaardAppStatus { is_default: Some(true), current: None, can_set: false, reason: Some("managed") }
}

/// The Linux status. `query`: the trimmed stdout of `xdg-mime query default
/// application/pdf` when xdg-mime ran; None when it is missing or failed.
/// In a sandbox the desktop's mime associations are out of reach (xdg-mime
/// is not in the runtime and could not write the host's mimeapps.list), so
/// the desktop's own settings are the way to choose the default there.
pub fn linux_status(sandbox: Option<&'static str>, query: Option<&str>) -> StandaardAppStatus {
    if let Some(s) = sandbox {
        return StandaardAppStatus { is_default: None, current: None, can_set: false, reason: Some(s) };
    }
    match query {
        None => StandaardAppStatus { is_default: None, current: None, can_set: false, reason: Some("xdg-mime-missing") },
        Some(id) => {
            let id = id.trim();
            StandaardAppStatus {
                is_default: Some(is_onze_desktop_id(id)),
                current: (!id.is_empty()).then(|| id.trim_end_matches(".desktop").to_string()),
                can_set: true,
                reason: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{beheerd_status, is_onze_desktop_id, linux_status, StandaardAppStatus};

    #[test]
    fn eigen_desktop_ids() {
        assert!(is_onze_desktop_id("HB PDF Studio.desktop\n"));
        assert!(is_onze_desktop_id("Open PDF Studio.desktop"));
        assert!(is_onze_desktop_id("org.openaec.OpenPdfStudio.desktop"));
        assert!(is_onze_desktop_id("open-pdf-studio_open-pdf-studio.desktop"));
        assert!(!is_onze_desktop_id("org.example.AndereLezer.desktop"));
        assert!(!is_onze_desktop_id(""));
    }

    #[test]
    fn linux_status_per_omgeving() {
        // Flatpak/Snap: niet na te gaan, niet te zetten; uitleg via reason.
        for s in ["flatpak", "snap"] {
            assert_eq!(
                linux_status(Some(s), Some("HB PDF Studio.desktop")),
                StandaardAppStatus { is_default: None, current: None, can_set: false, reason: Some(s) }
            );
        }
        assert_eq!(linux_status(None, None).reason, Some("xdg-mime-missing"));
        assert!(!linux_status(None, None).can_set);
        let ander = linux_status(None, Some("org.example.AndereLezer.desktop"));
        assert_eq!(ander.is_default, Some(false));
        assert_eq!(ander.current.as_deref(), Some("org.example.AndereLezer"));
        assert!(ander.can_set);
        let geen = linux_status(None, Some(""));
        assert_eq!((geen.is_default, geen.current), (Some(false), None));
        assert_eq!(linux_status(None, Some("open-pdf-studio.desktop")).is_default, Some(true));
        // JSON voor de JS-kant in camelCase.
        let json = serde_json::to_value(linux_status(Some("flatpak"), None)).unwrap();
        assert_eq!(json, serde_json::json!({"isDefault": null, "current": null, "canSet": false, "reason": "flatpak"}));
    }

    #[test]
    fn beheerde_modus_telt_als_standaard_zonder_te_kijken() {
        let s = beheerd_status();
        assert_eq!(s.is_default, Some(true), "geen balk bij het opstarten");
        assert!(!s.can_set);
        assert_eq!(s.reason, Some("managed"));
    }
}
