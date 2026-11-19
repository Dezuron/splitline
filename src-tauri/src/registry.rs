//! Direct access to the Windows registry.
//!
//! The app used to shell out to `reg.exe` for the WinINET proxy settings, the
//! device id and the autostart Run key. Spawning a process while the session is
//! shutting down is exactly what produced the "reg.exe — Ошибка приложения
//! (0xc0000142)" dialog: the loader cannot initialise a new process at logoff.
//! The registry API is a kernel call, so it works during shutdown as well.

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, KEY_READ, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE,
};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn open_key(root: HKEY, path: &str, access: u32, create: bool) -> Result<HKEY, String> {
    let subkey = wide(path);
    let mut key: HKEY = std::ptr::null_mut();
    let result = unsafe {
        if create {
            RegCreateKeyExW(
                root,
                subkey.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                access,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        } else {
            RegOpenKeyExW(root, subkey.as_ptr(), 0, access, &mut key)
        }
    };
    if result == ERROR_SUCCESS {
        Ok(key)
    } else {
        Err(format!(
            "Не удалось открыть раздел реестра {path} (код {result})."
        ))
    }
}

/// Reads a `REG_SZ` value. Returns `None` when the key or value is missing.
pub fn read_string(root: HKEY, path: &str, name: &str) -> Option<String> {
    let key = open_key(root, path, KEY_READ, false).ok()?;
    let value = wide(name);
    let mut kind: REG_VALUE_TYPE = 0;
    let mut size: u32 = 0;

    let probe = unsafe {
        RegQueryValueExW(
            key,
            value.as_ptr(),
            std::ptr::null(),
            &mut kind,
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if probe != ERROR_SUCCESS || size == 0 {
        unsafe { RegCloseKey(key) };
        return None;
    }

    // `size` is in bytes, including the terminating NUL for REG_SZ.
    let mut buffer = vec![0u8; size as usize];
    let read = unsafe {
        RegQueryValueExW(
            key,
            value.as_ptr(),
            std::ptr::null(),
            &mut kind,
            buffer.as_mut_ptr(),
            &mut size,
        )
    };
    unsafe { RegCloseKey(key) };
    if read != ERROR_SUCCESS {
        return None;
    }

    let units: Vec<u16> = buffer
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|unit| *unit != 0)
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// Reads a `REG_DWORD` value. Returns `None` when the key or value is missing.
pub fn read_dword(root: HKEY, path: &str, name: &str) -> Option<u32> {
    let key = open_key(root, path, KEY_READ, false).ok()?;
    let value = wide(name);
    let mut kind: REG_VALUE_TYPE = 0;
    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;

    let result = unsafe {
        RegQueryValueExW(
            key,
            value.as_ptr(),
            std::ptr::null(),
            &mut kind,
            &mut data as *mut u32 as *mut u8,
            &mut size,
        )
    };
    unsafe { RegCloseKey(key) };
    if result == ERROR_SUCCESS && kind == REG_DWORD {
        Some(data)
    } else {
        None
    }
}

/// Writes a `REG_SZ` value, creating the key when it does not exist yet.
pub fn write_string(root: HKEY, path: &str, name: &str, value: &str) -> Result<(), String> {
    let key = open_key(root, path, KEY_SET_VALUE, true)?;
    let name_wide = wide(name);
    let data: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = data.len() * std::mem::size_of::<u16>();

    let result = unsafe {
        RegSetValueExW(
            key,
            name_wide.as_ptr(),
            0,
            REG_SZ,
            data.as_ptr() as *const u8,
            bytes as u32,
        )
    };
    unsafe { RegCloseKey(key) };
    if result == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!(
            "Не удалось записать {path}\\{name} (код {result})."
        ))
    }
}

/// Writes a `REG_DWORD` value, creating the key when it does not exist yet.
pub fn write_dword(root: HKEY, path: &str, name: &str, value: u32) -> Result<(), String> {
    let key = open_key(root, path, KEY_SET_VALUE, true)?;
    let name_wide = wide(name);

    let result = unsafe {
        RegSetValueExW(
            key,
            name_wide.as_ptr(),
            0,
            REG_DWORD,
            &value as *const u32 as *const u8,
            std::mem::size_of::<u32>() as u32,
        )
    };
    unsafe { RegCloseKey(key) };
    if result == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!(
            "Не удалось записать {path}\\{name} (код {result})."
        ))
    }
}

/// Removes a value. A missing key or value is not an error.
pub fn delete_value(root: HKEY, path: &str, name: &str) -> bool {
    let Ok(key) = open_key(root, path, KEY_SET_VALUE, false) else {
        return false;
    };
    let name = wide(name);
    let result = unsafe { RegDeleteValueW(key, name.as_ptr()) };
    unsafe { RegCloseKey(key) };
    result == ERROR_SUCCESS
}

/// Removes an empty key. Used by the round-trip test to leave no trace.
#[cfg(test)]
fn delete_key(root: HKEY, path: &str) {
    use windows_sys::Win32::System::Registry::RegDeleteKeyW;
    let path = wide(path);
    unsafe {
        RegDeleteKeyW(root, path.as_ptr());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Registry::HKEY_CURRENT_USER;

    fn test_key() -> String {
        format!(r"Software\zapret-ui\registry-test-{}", std::process::id())
    }

    #[test]
    fn round_trips_strings_and_dwords() {
        let path = test_key();
        write_string(HKEY_CURRENT_USER, &path, "Text", "привет").expect("write string");
        write_dword(HKEY_CURRENT_USER, &path, "Flag", 1).expect("write dword");

        assert_eq!(
            read_string(HKEY_CURRENT_USER, &path, "Text").as_deref(),
            Some("привет")
        );
        assert_eq!(read_dword(HKEY_CURRENT_USER, &path, "Flag"), Some(1));

        assert!(delete_value(HKEY_CURRENT_USER, &path, "Text"));
        assert!(delete_value(HKEY_CURRENT_USER, &path, "Flag"));
        assert_eq!(read_string(HKEY_CURRENT_USER, &path, "Text"), None);
        assert_eq!(read_dword(HKEY_CURRENT_USER, &path, "Flag"), None);
        delete_key(HKEY_CURRENT_USER, &path);
    }

    #[test]
    fn missing_values_are_none() {
        assert_eq!(
            read_string(
                HKEY_CURRENT_USER,
                r"Software\zapret-ui\registry-test-missing",
                "Nope"
            ),
            None
        );
        assert_eq!(
            read_dword(
                HKEY_CURRENT_USER,
                r"Software\zapret-ui\registry-test-missing",
                "Nope"
            ),
            None
        );
    }
}
