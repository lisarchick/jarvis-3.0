//! Шифрование секретов через Windows DPAPI (CryptProtectData / CryptUnprotectData).
//! Формат на диске совместим с v1: `enc:v1:<base64>` и `plain:<...>` (запасной вариант).

use base64::Engine;

#[cfg(windows)]
mod imp {
    use super::*;
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};

    const PREFIX: &str = "enc:v1:";

    fn run_dpapi(data: &[u8], protect: bool) -> Option<Vec<u8>> {
        unsafe {
            let mut input = CRYPT_INTEGER_BLOB {
                cbData: data.len() as u32,
                pbData: data.as_ptr() as *mut u8,
            };
            let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
            let ok = if protect {
                CryptProtectData(&mut input, None, None, None, None, 0, &mut output).is_ok()
            } else {
                CryptUnprotectData(&mut input, None, None, None, None, 0, &mut output).is_ok()
            };
            if !ok {
                return None;
            }
            if output.pbData.is_null() || output.cbData == 0 {
                return Some(Vec::new());
            }
            let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            let _ = LocalFree(windows::Win32::Foundation::HLOCAL(output.pbData as *mut core::ffi::c_void));
            Some(bytes)
        }
    }

    pub fn encrypt(value: &str) -> String {
        if value.is_empty() {
            return String::new();
        }
        match run_dpapi(value.as_bytes(), true) {
            Some(blob) => format!("{}{}", PREFIX, base64::engine::general_purpose::STANDARD.encode(blob)),
            None => format!("plain:{}", value),
        }
    }

    pub fn decrypt(value: &str) -> String {
        if let Some(rest) = value.strip_prefix(PREFIX) {
            let blob = base64::engine::general_purpose::STANDARD.decode(rest).unwrap_or_default();
            run_dpapi(&blob, false).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
        } else if let Some(rest) = value.strip_prefix("plain:") {
            rest.to_string()
        } else {
            value.to_string()
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn encrypt(value: &str) -> String {
        if value.is_empty() { String::new() } else { format!("plain:{}", value) }
    }
    pub fn decrypt(value: &str) -> String {
        value.strip_prefix("plain:").unwrap_or(value).to_string()
    }
}

pub use imp::{decrypt, encrypt};
