// Cross-platform "is the microphone currently in use, and by which apps" detection.
//
// This distinguishes "a meeting app is merely running" from "a meeting is
// actually in progress" — the latter requires the microphone to be active.
//
// - macOS: query Core Audio process objects via cidre; `is_running_input()`
//   marks processes currently capturing audio input.
// - Windows: read the microphone CapabilityAccessManager consent store in the
//   registry; an app subkey with `LastUsedTimeStop == 0` is using the mic now.

/// Names of apps currently using the microphone (best-effort, may be empty even
/// if the mic is in use when the app identity can't be resolved).
#[derive(Debug, Clone, Default)]
pub struct MicUsage {
    pub in_use: bool,
    /// Lowercased identifiers of apps using the mic (bundle id / exe name / app name).
    pub app_identifiers: Vec<String>,
}

#[cfg(target_os = "macos")]
pub fn microphone_usage() -> MicUsage {
    use cidre::core_audio as ca;

    let processes = match ca::System::processes() {
        Ok(p) => p,
        Err(_) => return MicUsage::default(),
    };

    let mut app_identifiers = Vec::new();
    for process in processes {
        if process.is_running_input().unwrap_or(false) {
            if let Ok(pid) = process.pid() {
                if let Some(running_app) = cidre::ns::RunningApp::with_pid(pid) {
                    // Prefer bundle id (stable), fall back to localized name.
                    let ident = running_app
                        .bundle_id()
                        .map(|s| s.to_string())
                        .or_else(|| running_app.localized_name().map(|s| s.to_string()));
                    if let Some(ident) = ident {
                        app_identifiers.push(ident.to_lowercase());
                    }
                }
            }
        }
    }

    MicUsage {
        in_use: !app_identifiers.is_empty(),
        app_identifiers,
    }
}

#[cfg(target_os = "windows")]
pub fn microphone_usage() -> MicUsage {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    // Apps record mic usage under the consent store. A subkey whose
    // `LastUsedTimeStop` is 0 is *currently* using the microphone.
    // Non-packaged (desktop) apps live under the NonPackaged subkey.
    const STORE: &str =
        r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let mut app_identifiers = Vec::new();

    if let Ok(store) = hkcu.open_subkey(STORE) {
        collect_active_mic_apps(&store, &mut app_identifiers);

        if let Ok(nonpackaged) = store.open_subkey("NonPackaged") {
            collect_active_mic_apps(&nonpackaged, &mut app_identifiers);
        }
    }

    MicUsage {
        in_use: !app_identifiers.is_empty(),
        app_identifiers,
    }
}

#[cfg(target_os = "windows")]
fn collect_active_mic_apps(key: &winreg::RegKey, out: &mut Vec<String>) {
    for subkey_name in key.enum_keys().flatten() {
        // Skip the NonPackaged container itself; it's handled separately.
        if subkey_name.eq_ignore_ascii_case("NonPackaged") {
            continue;
        }
        if let Ok(app_key) = key.open_subkey(&subkey_name) {
            let last_used_stop: Result<u64, _> = app_key.get_value("LastUsedTimeStop");
            if let Ok(0) = last_used_stop {
                // 0 == still in use. The subkey name identifies the app
                // (packaged: PFN; non-packaged: exe path with # as separator).
                out.push(normalize_windows_app_key(&subkey_name));
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn normalize_windows_app_key(subkey_name: &str) -> String {
    // Non-packaged entries look like:
    //   C:#Program Files#Microsoft#Teams#current#Teams.exe
    // Convert '#' back to path separators and lowercase for matching.
    subkey_name.replace('#', "\\").to_lowercase()
}

// Other platforms (e.g. Linux): no mic-usage detection yet — report not-in-use
// so callers fall back to whatever other signals they have.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn microphone_usage() -> MicUsage {
    MicUsage::default()
}
