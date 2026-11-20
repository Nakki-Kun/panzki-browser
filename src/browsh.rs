use std::process::Command;

/// Build a Browsh command with environment tweaks for the Ubuntu Firefox snap.
///
/// The snap's Firefox has a private `/tmp` and cannot read the normal
/// `~/.config`, so Browsh's extension and profile must live inside
/// `~/snap/firefox/common`. Without this Browsh starts Firefox but fails to
/// install its WebExtension.
pub fn command(url: &str) -> Command {
    let mut command = Command::new("browsh");
    command.arg(url);
    if let Some(home) = dirs::home_dir() {
        let snap = home.join("snap/firefox/common");
        if snap.is_dir() {
            let config = snap.join(".config");
            let tmp = snap.join("tmp");
            let _ = std::fs::create_dir_all(&config);
            let _ = std::fs::create_dir_all(&tmp);

            // Browsh's Firefox profile often keeps a stale `.parentlock`
            // after Browsh exits, which makes the next launch fail with
            // "Firefox is already running". Remove it only when no Firefox
            // process is actually using the profile.
            let profile = config.join("browsh/firefox_profile");
            if !profile_in_use(&profile) {
                let _ = std::fs::remove_file(profile.join(".parentlock"));
                let _ = std::fs::remove_file(profile.join("lock"));
            }

            command.env("XDG_CONFIG_HOME", config);
            command.env("TMPDIR", tmp);
        }
    }
    command
}

/// Return true when a Firefox process is using Browsh's profile.
fn profile_in_use(profile: &std::path::Path) -> bool {
    let needle = profile.to_string_lossy();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    for entry in entries.flatten() {
        let cmdline = entry.path().join("cmdline");
        if let Ok(bytes) = std::fs::read(cmdline) {
            if String::from_utf8_lossy(&bytes).contains(needle.as_ref()) {
                return true;
            }
        }
    }
    false
}
