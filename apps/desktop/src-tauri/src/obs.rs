//! Detects OBS Studio starting, so Airlock can open its listener with it.
//!
//! The image name is `obs64.exe` on 64-bit Windows. A second copy of an
//! already-running OBS is not a new launch.

use std::process::Command;

pub fn is_obs_process(name: &str) -> bool {
    let file = name.rsplit(['\\', '/']).next().unwrap_or(name);
    matches!(
        file.to_ascii_lowercase().as_str(),
        "obs64.exe" | "obs32.exe" | "obs"
    )
}

pub fn obs_just_launched(was_running: bool, running: bool) -> bool {
    !was_running && running
}

pub fn process_names_from_tasklist(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix('"')?;
            let end = rest.find('"')?;
            Some(rest[..end].to_string())
        })
        .collect()
}

pub fn snapshot_has_obs(names: &[String]) -> bool {
    names.iter().any(|name| is_obs_process(name))
}

pub fn obs_is_running() -> bool {
    #[cfg(windows)]
    {
        windows_process_list_has_obs()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
fn windows_process_list_has_obs() -> bool {
    let Ok(output) = Command::new("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    snapshot_has_obs(&process_names_from_tasklist(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obs64_is_a_launch_and_other_images_are_not() {
        assert!(is_obs_process("obs64.exe"));
        assert!(is_obs_process(
            r"C:\Program Files\obs-studio\bin\64bit\obs64.exe"
        ));
        assert!(is_obs_process("OBS64.EXE"));
        assert!(!is_obs_process("chrome.exe"));
        assert!(!is_obs_process("obs-websocket.exe"));
        assert!(obs_just_launched(false, true));
        assert!(!obs_just_launched(true, true));
        assert!(!obs_just_launched(true, false));
        let names = process_names_from_tasklist(
            "\"obs64.exe\",\"1234\",\"Console\",\"1\",\"120,000 K\"\n\"chrome.exe\",\"9\",\"Console\",\"1\",\"1 K\"\n",
        );
        assert!(snapshot_has_obs(&names));
        let quiet =
            process_names_from_tasklist("\"notepad.exe\",\"1\",\"Console\",\"1\",\"1 K\"\n");
        assert!(!snapshot_has_obs(&quiet));
    }

    #[test]
    #[ignore = "needs obs64.exe running"]
    fn a_running_obs_is_visible() {
        assert!(obs_is_running());
    }
}
