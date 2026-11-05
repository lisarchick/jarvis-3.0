//! Нативная автоматизация Windows: медиа-клавиши, громкость, запуск приложений,
//! поиск по меню «Пуск», управление окнами. Всё через WinAPI без промежуточных процессов.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use once_cell::sync::Lazy;

// ------------------------------------------------------------------ медиа-клавиши

const VK_MUTE: u16 = 0xAD;
const VK_VOL_DOWN: u16 = 0xAE;
const VK_VOL_UP: u16 = 0xAF;
const VK_NEXT: u16 = 0xB0;
const VK_PREV: u16 = 0xB1;
const VK_STOP: u16 = 0xB2;
const VK_PLAY_PAUSE: u16 = 0xB3;

#[cfg(windows)]
fn press_key(vk: u16) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY,
    };
    let make = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inputs = [make(KEYBD_EVENT_FLAGS(0)), make(KEYEVENTF_KEYUP)];
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    sent == 2
}

#[cfg(not(windows))]
fn press_key(_vk: u16) -> bool {
    false
}

pub fn media_key(name: &str, times: u32) -> bool {
    let vk = match name {
        "mute" => VK_MUTE,
        "vol_down" => VK_VOL_DOWN,
        "vol_up" => VK_VOL_UP,
        "next" => VK_NEXT,
        "prev" => VK_PREV,
        "stop" => VK_STOP,
        "play_pause" => VK_PLAY_PAUSE,
        _ => return false,
    };
    for _ in 0..times.max(1) {
        if !press_key(vk) {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    true
}

// ------------------------------------------------------------------ громкость (IAudioEndpointVolume)

#[cfg(windows)]
fn with_endpoint<R>(f: impl Fn(&windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume) -> R) -> Option<R> {
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{eRender, eConsole, IMMDeviceEnumerator, MMDeviceEnumerator};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED};

    unsafe {
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        let need_uninit = hr.is_ok();
        let result = (|| -> Option<R> {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
            let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
            Some(f(&endpoint))
        })();
        if need_uninit {
            CoUninitialize();
        }
        result
    }
}

#[cfg(not(windows))]
fn with_endpoint<R>(_f: impl Fn(&()) -> R) -> Option<R> {
    None
}

pub fn get_volume() -> Option<u32> {
    #[cfg(windows)]
    {
        with_endpoint(|ep| unsafe { ep.GetMasterVolumeLevelScalar().ok() })
            .flatten()
            .map(|v: f32| (v.clamp(0.0, 1.0) * 100.0).round() as u32)
    }
    #[cfg(not(windows))]
    {
        let _ = with_endpoint::<()>(|_| ());
        None
    }
}

pub fn set_volume(percent: i64) -> bool {
    let percent = percent.clamp(0, 100) as f32 / 100.0;
    #[cfg(windows)]
    {
        let ok = with_endpoint(|ep| unsafe {
            ep.SetMute(false, std::ptr::null()).ok()?;
            ep.SetMasterVolumeLevelScalar(percent, std::ptr::null()).ok()?;
            Some(())
        })
        .flatten();
        if ok.is_some() {
            return true;
        }
    }
    // Запасной путь — клавиши громкости.
    media_key("vol_down", 50);
    media_key("vol_up", (percent * 100.0) as u32 / 2)
}

pub fn set_mute(mute: bool) -> bool {
    #[cfg(windows)]
    {
        let ok = with_endpoint(|ep| unsafe { ep.SetMute(mute, std::ptr::null()).ok() }).flatten();
        if ok.is_some() {
            return true;
        }
    }
    media_key("mute", 1)
}

/// Тройной сигнал таймера: 1 кГц, генерируем WAV и играем в отдельном потоке.
pub fn beep() {
    std::thread::spawn(|| {
        let wav = tone_wav(1000.0, 0.22, 3, 0.10);
        audio_play(&wav);
    });
}

/// Простой WAV: моно 16 кГц, `count` гудков частоты `freq` с паузами.
fn tone_wav(freq: f32, dur_sec: f32, count: u32, gap_sec: f32) -> Vec<u8> {
    let rate = 16000u32;
    let tone = (rate as f32 * dur_sec) as usize;
    let gap = (rate as f32 * gap_sec) as usize;
    let total = (tone + gap) * count as usize;
    let mut samples: Vec<i16> = Vec::with_capacity(total);
    for _n in 0..count {
        for i in 0..tone {
            let t = i as f32 / rate as f32;
            // короткая атака/затухание, чтобы не щёлкало
            let env = ((t / dur_sec) * (1.0 - t / dur_sec) * 8.0).min(1.0);
            let v = (2.0 * std::f32::consts::PI * freq * t).sin() * env * 0.42;
            samples.push((v * i16::MAX as f32) as i16);
        }
        samples.extend(std::iter::repeat(0i16).take(gap));
    }
    let data_len = samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

// ------------------------------------------------------------------ запуск программ и сайтов

/// Открывает URL/URI/путь/исполняемый файл — как os.startfile в v1.
pub fn shell_open(target: &str) -> bool {
    #[cfg(windows)]
    {
        use windows::core::PCWSTR;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let wide: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
        let verb: Vec<u16> = "open\0".encode_utf16().collect();
        let result = unsafe {
            ShellExecuteW(None, PCWSTR(verb.as_ptr()), PCWSTR(wide.as_ptr()), None, None, SW_SHOWNORMAL)
        };
        result.0 as usize > 32
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("xdg-open").arg(target).spawn().is_ok()
    }
}

/// Ищет exe через PATH и раздел реестра App Paths.
pub fn resolve_exe(name: &str) -> Option<String> {
    let exe = if name.to_ascii_lowercase().ends_with(".exe") { name.to_string() } else { format!("{}.exe", name) };
    // PATH
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(';') {
            let candidate = PathBuf::from(dir).join(&exe);
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    // System32 (notepad, calc, mspaint…)
    if let Ok(system_root) = std::env::var("SystemRoot") {
        let candidate = PathBuf::from(&system_root).join("system32").join(&exe);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    // App Paths
    #[cfg(windows)]
    {
        use windows::core::HSTRING;
        use windows::Win32::System::Registry::{RegQueryValueExW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RegOpenKeyExW, KEY_READ, REG_VALUE_TYPE};
        for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
            let subkey = format!("Software\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{}", exe);
            let subkey_h: HSTRING = subkey.into();
            let mut hkey = Default::default();
            if unsafe { RegOpenKeyExW(root, &subkey_h, 0, KEY_READ, &mut hkey) }.is_err() {
                continue;
            }
            let name_h: HSTRING = "".into();
            let mut buf = [0u16; 1024];
            let mut len = 1024u32;
            let mut ty = REG_VALUE_TYPE(0);
            let ok = unsafe {
                RegQueryValueExW(hkey, &name_h, None, Some(&mut ty), Some(buf.as_mut_ptr().cast()), Some(&mut len))
            };
            let _ = unsafe { windows::Win32::System::Registry::RegCloseKey(hkey) };
            if ok.is_ok() {
                let s = String::from_utf16_lossy(&buf[..((len / 2) as usize).saturating_sub(1).min(1023)]);
                let s = s.trim().trim_matches('"').to_string();
                if PathBuf::from(&s).is_file() {
                    return Some(s);
                }
            }
        }
    }
    None
}

static START_MENU: Lazy<Mutex<Option<HashMap<String, String>>>> = Lazy::new(|| Mutex::new(None));

/// Индекс ярлыков меню «Пуск»: {название_нижним_регистром: путь к .lnk}.
pub fn start_menu_index() -> HashMap<String, String> {
    let mut cache = START_MENU.lock().unwrap();
    if cache.is_none() {
        let mut map = HashMap::new();
        let roots = [
            std::env::var("ProgramData").map(|p| PathBuf::from(p).join(r"Microsoft\Windows\Start Menu\Programs")).ok(),
            std::env::var("APPDATA").map(|p| PathBuf::from(p).join(r"Microsoft\Windows\Start Menu\Programs")).ok(),
        ];
        for root in roots.into_iter().flatten() {
            index_lnk_dir(&root, &mut map);
        }
        *cache = Some(map);
    }
    cache.as_ref().unwrap().clone()
}

fn index_lnk_dir(dir: &PathBuf, map: &mut HashMap<String, String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            index_lnk_dir(&path, map);
        } else if path.extension().map(|e| e.eq_ignore_ascii_case("lnk")).unwrap_or(false) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                map.entry(stem.to_lowercase()).or_insert_with(|| path.to_string_lossy().into_owned());
            }
        }
    }
}

// ------------------------------------------------------------------ воспроизведение WAV (rodio 0.23: Mixer + Player)

use rodio::buffer::SamplesBuffer;
use rodio::mixer::Mixer;
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player};

/// Устройство вывода живёт всё время работы приложения.
static AUDIO: Lazy<Mutex<Option<(MixerDeviceSink, Mixer)>>> = Lazy::new(|| Mutex::new(None));
static PLAYER: Lazy<Mutex<Option<std::sync::Arc<Player>>>> = Lazy::new(|| Mutex::new(None));

fn ensure_mixer() -> Option<Mixer> {
    let mut guard = AUDIO.lock().unwrap();
    if guard.is_none() {
        match DeviceSinkBuilder::open_default_sink() {
            Ok(sink) => {
                let mixer = sink.mixer().clone();
                *guard = Some((sink, mixer.clone()));
                return Some(mixer);
            }
            Err(e) => {
                eprintln!("[audio] нет устройства вывода: {}", e);
                return None;
            }
        }
    }
    guard.as_ref().map(|(_, m)| m.clone())
}

/// Играет WAV-байты; блокируется до конца звучания. audio_stop() прерывает.
pub fn audio_play(wav: &[u8]) {
    let Some(mixer) = ensure_mixer() else { return };
    let source = match wav_to_source(wav) {
        Some(s) => s,
        None => {
            eprintln!("[audio] не удалось разобрать WAV");
            return;
        }
    };
    let player = Player::connect_new(&mixer);
    player.append(source);
    let shared = std::sync::Arc::new(player);
    *PLAYER.lock().unwrap() = Some(shared.clone());
    // Ждём конца; stop() из другого потока прерывает ожидание.
    shared.sleep_until_end();
    *PLAYER.lock().unwrap() = None;
}

/// Мгновенная остановка воспроизведения (barge-in).
pub fn audio_stop() {
    let player = PLAYER.lock().unwrap().clone();
    if let Some(p) = player {
        p.stop();
    }
}

/// WAV (PCM 16-bit) → rodio-источник f32.
fn wav_to_source(wav: &[u8]) -> Option<SamplesBuffer> {
    if wav.len() < 44 {
        return None;
    }
    let rd = |off: usize| u32::from_le_bytes([wav[off], wav[off + 1], wav[off + 2], wav[off + 3]]);
    let rd16 = |off: usize| u16::from_le_bytes([wav[off], wav[off + 1]]);
    if &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return None;
    }
    let channels = rd16(22);
    let rate = rd(24);
    if channels == 0 || rate == 0 {
        return None;
    }
    // Ищем data-чанк (fmt обычно 16 байт, но не всегда).
    let mut pos = 12usize;
    let mut data: &[u8] = &[];
    while pos + 8 <= wav.len() {
        let id = &wav[pos..pos + 4];
        let size = rd(pos + 4) as usize;
        if id == b"data" {
            data = &wav[pos + 8..(pos + 8 + size).min(wav.len())];
            break;
        }
        pos += 8 + size + (size & 1);
    }
    if data.is_empty() {
        return None;
    }
    let samples: Vec<f32> = data
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / i16::MAX as f32)
        .collect();
    let ch = std::num::NonZeroU16::new(channels)?;
    let sr = std::num::NonZeroU32::new(rate)?;
    Some(SamplesBuffer::new(ch, sr, samples))
}

// ------------------------------------------------------------------ окна

#[derive(serde::Serialize)]
pub struct WindowInfo {
    pub title: String,
    pub process: String,
    pub hwnd: isize,
}

/// Список видимых окон верхнего уровня с заголовками.
pub fn list_windows() -> Vec<WindowInfo> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::LPARAM;
        let mut result: Vec<WindowInfo> = Vec::new();
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::EnumWindows(
                Some(enum_cb),
                LPARAM(&mut result as *mut Vec<WindowInfo> as isize),
            );
        }
        return result;
    }
    #[cfg(not(windows))]
    Vec::new()
}

#[cfg(windows)]
unsafe extern "system" fn enum_cb(hwnd: windows::Win32::Foundation::HWND, lparam: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::BOOL {
    use windows::Win32::Foundation::BOOL;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowLongW, GetWindowTextLengthW, GetWindowTextW, GWL_STYLE, IsWindowVisible, WS_MINIMIZE, WS_VISIBLE};
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        if style & WS_VISIBLE.0 == 0 {
            return BOOL(1);
        }
        let len = GetWindowTextLengthW(hwnd);
        if len == 0 {
            return BOOL(1);
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        GetWindowTextW(hwnd, &mut buf);
        let title = String::from_utf16_lossy(&buf[..len as usize]);
        if title.trim().is_empty() {
            return BOOL(1);
        }
        let process = window_process_name(hwnd);
        let list = &mut *(lparam.0 as *mut Vec<WindowInfo>);
        list.push(WindowInfo { title, process, hwnd: hwnd.0 as isize });
        let _ = WS_MINIMIZE;
    }
    BOOL(1)
}

#[cfg(windows)]
fn window_process_name(hwnd: windows::Win32::Foundation::HWND) -> String {
    use windows::Win32::Foundation::CloseHandle;
    unsafe {
        let mut pid = 0u32;
        windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return String::new();
        }
        let handle = windows::Win32::System::Threading::OpenProcess(
            windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        );
        let Ok(handle) = handle else { return String::new() };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = windows::Win32::System::Threading::QueryFullProcessImageNameW(
            handle,
            windows::Win32::System::Threading::PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(handle);
        if ok.is_err() {
            return String::new();
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        std::path::Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// Переводит окно на передний план.
pub fn focus_window(hwnd: isize) -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW};
        let hwnd = HWND(hwnd as *mut core::ffi::c_void);
        unsafe {
            if IsIconicHandle(hwnd) {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            } else {
                let _ = ShowWindow(hwnd, SW_SHOW);
            }
            SetForegroundWindow(hwnd).as_bool()
        }
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        false
    }
}

#[cfg(windows)]
fn IsIconicHandle(hwnd: windows::Win32::Foundation::HWND) -> bool {
    unsafe { windows::Win32::UI::WindowsAndMessaging::IsIconic(hwnd).as_bool() }
}

/// Закрывает окно (WM_CLOSE).
pub fn close_window(hwnd: isize) -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
        let hwnd = HWND(hwnd as *mut core::ffi::c_void);
        unsafe { PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok() }
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        false
    }
}

/// Сворачивает все окна (Win+D).
pub fn minimize_all() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_LWIN,
        };
        let make = |vk: VIRTUAL_KEY, up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let inputs = [
            make(VK_LWIN, false),
            make(VIRTUAL_KEY(0x44), false), // D
            make(VIRTUAL_KEY(0x44), true),
            make(VK_LWIN, true),
        ];
        let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        sent == 4
    }
    #[cfg(not(windows))]
    false
}

// ------------------------------------------------------------------ Recycle Bin / клипборд / блокировка

/// Удаляет файл/папку в корзину (SHFileOperationW + FOF_ALLOWUNDO). Только корзина.
pub fn recycle_delete(path: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows::core::PCWSTR;
        use windows::Win32::UI::Shell::{SHFileOperationW, SHFILEOPSTRUCTW, FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT};
        let mut wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).chain(std::iter::once(0)).collect();
        let flags = (FOF_ALLOWUNDO.0 | FOF_NOCONFIRMATION.0 | FOF_SILENT.0 | FOF_NOERRORUI.0) as u16;
        let mut op = SHFILEOPSTRUCTW {
            hwnd: windows::Win32::Foundation::HWND::default(),
            wFunc: FO_DELETE,
            pFrom: PCWSTR(wide.as_mut_ptr()),
            pTo: PCWSTR::null(),
            fFlags: flags,
            fAnyOperationsAborted: windows::Win32::Foundation::BOOL::default(),
            hNameMappings: std::ptr::null_mut(),
            lpszProgressTitle: PCWSTR::null(),
        };
        let res = unsafe { SHFileOperationW(&mut op) };
        if res != 0 {
            return Err(format!("не удалось удалить в корзину (код {})", res));
        }
        if op.fAnyOperationsAborted.as_bool() {
            return Err("удаление отменено".into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    Err("удаление в корзину поддерживается только на Windows".into())
}

/// Текст буфера обмена (или пустая строка).
pub fn clipboard_get() -> String {
    #[cfg(windows)]
    {
        const CF_UNICODETEXT: u32 = 13;
        use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
        use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
        unsafe {
            if OpenClipboard(None).is_err() {
                return String::new();
            }
            let mut out = String::new();
            if let Ok(handle) = GetClipboardData(CF_UNICODETEXT) {
                if !handle.is_invalid() {
                    let ptr = GlobalLock(windows::Win32::Foundation::HGLOBAL(handle.0)) as *const u16;
                    if !ptr.is_null() {
                        let mut len = 0usize;
                        while *ptr.add(len) != 0 {
                            len += 1;
                        }
                        out = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
                        let _ = GlobalUnlock(windows::Win32::Foundation::HGLOBAL(handle.0));
                    }
                }
            }
            let _ = CloseClipboard();
            out
        }
    }
    #[cfg(not(windows))]
    String::new()
}

/// Кладёт текст в буфер обмена.
pub fn clipboard_set(text: &str) -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
        use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
        unsafe {
            if OpenClipboard(None).is_err() {
                return false;
            }
            let _ = EmptyClipboard();
            let mut wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes = wide.len() * 2;
            let Ok(hglobal) = GlobalAlloc(GMEM_MOVEABLE, bytes) else {
                let _ = CloseClipboard();
                return false;
            };
            let ptr = GlobalLock(hglobal) as *mut u16;
            if ptr.is_null() {
                let _ = CloseClipboard();
                return false;
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
            let _ = GlobalUnlock(hglobal);
            let ok = SetClipboardData(13, windows::Win32::Foundation::HANDLE(hglobal.0)).is_ok(); // CF_UNICODETEXT
            let _ = CloseClipboard();
            ok
        }
    }
    #[cfg(not(windows))]
    {
        let _ = text;
        false
    }
}

/// Блокирует экран (LockWorkStation).
pub fn lock_workstation() -> bool {
    #[cfg(windows)]
    {
        unsafe { windows::Win32::System::Shutdown::LockWorkStation() }.is_ok()
    }
    #[cfg(not(windows))]
    false
}

// ------------------------------------------------------------------ системная папка

/// Пути, где разрешены операции с файлами по умолчанию.
pub fn user_allowed_roots(extra: &[String]) -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();
    if let Ok(home) = std::env::var("USERPROFILE") {
        let h = std::path::PathBuf::from(&home);
        for sub in ["Desktop", "Documents", "Downloads", "Music", "Pictures", "Videos"] {
            roots.push(h.join(sub));
        }
        // Загрузки могут быть перенесены — не критично.
    }
    for e in extra {
        let p = std::path::PathBuf::from(e);
        if p.is_dir() {
            roots.push(p);
        }
    }
    roots
}

/// Проверяет путь на безопасность удаления: внутри разрешённых корней,
/// не системная директория, не корень диска.
pub fn path_guard(path: &str, extra: &[String]) -> Result<std::path::PathBuf, String> {
    let p = std::path::PathBuf::from(path);
    let canonical = if p.exists() {
        p.canonicalize().map_err(|e| format!("путь недоступен: {}", e))?
    } else {
        return Err("путь не существует".into());
    };
    let s = canonical.to_string_lossy().to_lowercase();

    // Запрещённые зоны: системные.
    for forbidden in [
        r"c:\windows", r"\program files", r"\program files (x86)", r"\programdata\microsoft",
        r"c:\users\public", r"\appdata\local\temp",
    ] {
        if s.contains(forbidden) {
            return Err(format!("системный путь защищён: {}", forbidden));
        }
    }
    // Корень диска и корень профиля нельзя.
    let comps: Vec<_> = canonical.components().collect();
    if comps.len() <= 2 {
        return Err("нельзя удалять корень диска".into());
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        let home_low = home.to_lowercase().replace(r"\", "/");
        let s_norm = s.replace(r"\", "/");
        if s_norm == home_low {
            return Err("нельзя удалять домашнюю папку целиком".into());
        }
        // Прямые дети профиля (кроме разрешённых ниже) не удаляем целиком.
        let depth_home = home_low.split('/').count();
        let depth_path = s_norm.split('/').count();
        if s_norm.starts_with(&home_low) && depth_path <= depth_home + 1 {
            let allowed_names = ["desktop", "documents", "downloads", "music", "pictures", "videos"];
            let last = s_norm.rsplit('/').next().unwrap_or("");
            if !allowed_names.contains(&last) {
                return Err("нельзя удалять папки верхнего уровня профиля".into());
            }
        }
    }
    // Разрешённые корни.
    for root in user_allowed_roots(extra) {
        let r = root.canonicalize().unwrap_or(root).to_string_lossy().to_lowercase().replace(r"\", "/");
        if s.replace(r"\", "/").starts_with(&r) {
            return Ok(canonical);
        }
    }
    Err("путь вне разрешённых папок (добавь папку в настройках)".into())
}
