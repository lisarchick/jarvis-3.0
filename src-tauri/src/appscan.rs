//! Сканер установленных приложений: меню «Пуск», ярлыки рабочего стола,
//! раздел реестра Uninstall, UWP-пакеты, известные музыкальные приложения.
//! Запускается только после явного разрешения (app_scan). Всё локально.

use std::path::PathBuf;

/// Известные музыкальные приложения (для умного роутинга плеера).
const MEDIA_APPS: &[(&str, &str)] = &[
    ("spotify", "uri:spotify:"),
    ("vlc", "exe:vlc.exe"),
    ("yandex music", "uri:yandexmusic://"),
    ("soundcloud", "uri:https://soundcloud.com"),
    ("aimp", "exe:AIMP.exe"),
    ("foobar2000", "exe:foobar2000.exe"),
    ("winamp", "exe:winamp.exe"),
    ("itunes", "exe:iTunes.exe"),
    ("mediamonkey", "exe:MediaMonkey.exe"),
    ("musicbee", "exe:MusicBee.exe"),
];

pub struct ScannedApp {
    pub name: String,
    pub path: String,
    pub kind: String, // exe | lnk | uri
    pub is_media: bool,
}

/// Полный скан. Возвращает индекс для SQLite.
pub fn scan_all() -> Vec<ScannedApp> {
    let mut out: Vec<ScannedApp> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let push = |out: &mut Vec<ScannedApp>, seen: &mut std::collections::HashSet<String>, name: String, path: String, kind: &str, is_media: bool| {
        let key = name.to_lowercase();
        if key.is_empty() || seen.contains(&key) {
            return;
        }
        seen.insert(key);
        out.push(ScannedApp { name, path, kind: kind.to_string(), is_media });
    };

    // 1. Меню «Пуск» (все пользователи + текущий)
    for root in start_menu_roots() {
        walk_lnk(&root, &mut |name, path| {
            let media = MEDIA_APPS.iter().any(|(m, _)| name.contains(m));
            push(&mut out, &mut seen, name, path, "lnk", media);
        });
    }

    // 2. Ярлыки рабочего стола
    if let Ok(desktop) = std::env::var("USERPROFILE") {
        let d = PathBuf::from(&desktop).join("Desktop");
        walk_lnk(&d, &mut |name, path| push(&mut out, &mut seen, name, path, "lnk", false));
        let public = PathBuf::from(r"C:\Users\Public\Desktop");
        walk_lnk(&public, &mut |name, path| push(&mut out, &mut seen, name, path, "lnk", false));
    }

    // 3. Реестр: Uninstall-записи (HKCU + HKLM, 64/32)
    #[cfg(windows)]
    for (root, subkeys) in uninstall_roots() {
        for sub in subkeys {
            enumerate_uninstall(root, &sub, &mut |name, path| {
                push(&mut out, &mut seen, name, path, "exe", false);
            });
        }
    }

    // 4. UWP: %LOCALAPPDATA%\Packages → известные пакеты по имени.
    #[cfg(windows)]
    scan_uwp(&mut |name, path| {
        push(&mut out, &mut seen, name, path, "uwp", false);
    });

    // 5. Известные музыкальные приложения: есть ли на диске.
    for (name, spec) in MEDIA_APPS {
        let (kind, target) = match spec.split_once(':') {
            Some(("uri", u)) => ("uri", u.to_string()),
            Some(("exe", e)) => ("exe", e.to_string()),
            _ => continue,
        };
        let installed = if kind == "uri" {
            name != &"soundcloud" || true // веб-плеер доступен всегда
        } else {
            crate::os::resolve_exe(&target).is_some() || out.iter().any(|a| a.name.contains(name))
        };
        if installed {
            let target = if kind == "exe" { crate::os::resolve_exe(&target).unwrap_or(target) } else { target };
            push(&mut out, &mut seen, name.to_string(), target, kind, true);
        }
    }

    // 6. Браузер по умолчанию — через ассоциацию http (UserChoice ProgId).
    if let Some(browser) = default_browser() {
        push(&mut out, &mut seen, "браузер".into(), browser.1, "exe", false);
    }

    out
}

fn start_menu_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(pd) = std::env::var("ProgramData") {
        roots.push(PathBuf::from(pd).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Ok(ad) = std::env::var("APPDATA") {
        roots.push(PathBuf::from(ad).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    roots
}

fn walk_lnk(dir: &PathBuf, f: &mut dyn FnMut(String, String)) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_lnk(&path, f);
        } else if path.extension().map(|e| e.eq_ignore_ascii_case("lnk")).unwrap_or(false) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if !stem.eq_ignore_ascii_case("desktop") && !stem.eq_ignore_ascii_case("uninstall") {
                    f(stem.to_string(), path.to_string_lossy().into_owned());
                }
            }
        }
    }
}

#[cfg(windows)]
fn uninstall_roots() -> Vec<(windows::Win32::System::Registry::HKEY, Vec<&'static str>)> {
    use windows::Win32::System::Registry::HKEY_CURRENT_USER;
    use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    vec![
        (HKEY_CURRENT_USER, vec![r"Software\Microsoft\Windows\CurrentVersion\Uninstall"]),
        (
            HKEY_LOCAL_MACHINE,
            vec![
                r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
                r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
            ],
        ),
    ]
}

#[cfg(windows)]
fn enumerate_uninstall(
    root: windows::Win32::System::Registry::HKEY,
    subkey: &str,
    f: &mut dyn FnMut(String, String),
) {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegQueryValueExW, RegOpenKeyExW, KEY_READ, REG_VALUE_TYPE,
    };
    unsafe {
        let sub: HSTRING = subkey.into();
        let mut hkey = Default::default();
        if RegOpenKeyExW(root, &sub, 0, KEY_READ, &mut hkey).is_err() {
            return;
        }
        let mut idx = 0u32;
        loop {
            let mut name_buf = [0u16; 256];
            let mut len = 256u32;
            let res = RegEnumKeyExW(hkey, idx, windows::core::PWSTR(name_buf.as_mut_ptr()), &mut len, None, windows::core::PWSTR::null(), None, None);
            if res.is_err() {
                break;
            }
            idx += 1;
            let child: String = String::from_utf16_lossy(&name_buf[..len as usize]);
            let child_path = format!("{}\\{}", subkey, child);
            let ch: HSTRING = child_path.into();
            let mut ck = Default::default();
            if RegOpenKeyExW(root, &ch, 0, KEY_READ, &mut ck).is_err() {
                continue;
            }
            let read = |val: &str| -> Option<String> {
                let vh: HSTRING = val.into();
                let mut buf = [0u16; 1024];
                let mut vlen = 2048u32;
                let mut vty = REG_VALUE_TYPE(0);
                if RegQueryValueExW(ck, &vh, None, Some(&mut vty), Some(buf.as_mut_ptr().cast()), Some(&mut vlen)).is_ok()
                    && vty.0 == 1
                {
                    Some(String::from_utf16_lossy(&buf[..((vlen / 2) as usize).saturating_sub(1).min(1023)]).trim().to_string())
                } else {
                    None
                }
            };
            let display = read("DisplayName");
            let location = read("InstallLocation").filter(|p| !p.is_empty());
            let icon = read("DisplayIcon");
            if let Some(display) = display {
                if display.to_lowercase().contains("uninstall") {
                    continue;
                }
                // Путь: InstallLocation + exe, либо DisplayIcon без ,0
                let path = location
                    .map(|l| {
                        let exe = format!("{}.exe", display.replace(' ', ""));
                        let guess = PathBuf::from(&l).join(&exe);
                        if guess.exists() { guess.to_string_lossy().into_owned() } else { l }
                    })
                    .or_else(|| icon.map(|i| i.split(',').next().unwrap_or(&i).trim_matches('"').to_string()))
                    .unwrap_or_default();
                if !path.is_empty() {
                    f(display, path);
                }
            }
            let _ = RegCloseKey(ck);
        }
        let _ = RegCloseKey(hkey);
    }
}

#[cfg(windows)]
fn scan_uwp(f: &mut dyn FnMut(String, String)) {
    // shell:AppsFolder — прямые URI запуска известные пакеты; без WinRT-перебора
    // берём только реально установленные известные пакеты по папкам.
    let Ok(local) = std::env::var("LOCALAPPDATA") else { return };
    let packages = PathBuf::from(&local).join("Packages");
    let known: &[(&str, &str)] = &[
        ("SpotifyAB.SpotifyMusic", "spotify"),
        ("AppleInc.iTunes", "itunes"),
    ];
    for (pkg, nice) in known {
        if packages.join(pkg).is_dir() {
            f(nice.to_string(), format!("shell:AppsFolder\\{}!App", pkg));
        }
    }
}

fn default_browser() -> Option<(String, String)> {
    #[cfg(windows)]
    {
        use windows::core::HSTRING;
        use windows::Win32::System::Registry::{RegQueryValueExW, RegOpenKeyExW, KEY_READ, HKEY_CURRENT_USER, REG_VALUE_TYPE};
        unsafe {
            let sub: HSTRING = r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\http\UserChoice".into();
            let mut hk = Default::default();
            if RegOpenKeyExW(HKEY_CURRENT_USER, &sub, 0, KEY_READ, &mut hk).is_err() {
                return None;
            }
            let vh: HSTRING = "ProgId".into();
            let mut buf = [0u16; 512];
            let mut len = 1024u32;
            let mut ty = REG_VALUE_TYPE(0);
            let ok = RegQueryValueExW(hk, &vh, None, Some(&mut ty), Some(buf.as_mut_ptr().cast()), Some(&mut len));
            let _ = windows::Win32::System::Registry::RegCloseKey(hk);
            if ok.is_err() {
                return None;
            }
            let progid = String::from_utf16_lossy(&buf[..((len / 2) as usize).saturating_sub(1).min(511)]);
            let nice = match () {
                _ if progid.starts_with("ChromeHTML") => "Google Chrome",
                _ if progid.starts_with("MSEdgeHTM") => "Microsoft Edge",
                _ if progid.to_lowercase().contains("firefox") => "Firefox",
                _ => "браузер",
            };
            return Some((nice.to_string(), progid));
        }
    }
    #[cfg(not(windows))]
    None
}
