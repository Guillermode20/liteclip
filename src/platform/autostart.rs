//! Windows auto-start registry management
//!
//! Manages the `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` registry
//! entry so LiteClip can optionally launch on Windows startup.

use anyhow::{Context, Result};
use tracing::info;
#[cfg(not(windows))]
use tracing::warn;

#[cfg(windows)]
const REG_APP_NAME: &str = "LiteClip";

/// Enable or disable Windows auto-start for the current executable.
///
/// On Windows this writes/removes a value under
/// `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
///
/// On non-Windows platforms this is a no-op (returns `Ok(())`).
pub fn set_autostart(enabled: bool) -> Result<()> {
    #[cfg(windows)]
    {
        use windows::core::PCWSTR;
        use windows::Win32::System::Registry::{
            RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY_CURRENT_USER,
            KEY_SET_VALUE, REG_SZ,
        };

        // Encode the sub-key and value name as null-terminated wide strings
        let subkey_wide: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Run\0"
            .encode_utf16()
            .collect();
        let value_name_wide: Vec<u16> = format!("{}\0", REG_APP_NAME).encode_utf16().collect();

        // Open the registry key — RegOpenKeyExW returns WIN32_ERROR, convert with .ok()
        let mut hkey = windows::Win32::System::Registry::HKEY::default();
        unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey_wide.as_ptr()),
                0,
                KEY_SET_VALUE,
                &mut hkey,
            )
            .ok()
        }
        .context("Failed to open HKCU\\...\\Run registry key")?;

        let result = if enabled {
            let exe_path = std::env::current_exe()
                .context("Failed to get current executable path for autostart")?;

            let exe_path_str = exe_path.to_string_lossy();
            let is_installed = exe_path_str.to_lowercase().contains("program files");

            if !is_installed {
                // Close the registry key before early return to prevent handle leak
                unsafe {
                    let _ = RegCloseKey(hkey).ok();
                };
                info!(
                    "Skipping auto-start setup: not running from installed location (path: {:?})",
                    exe_path
                );
                return Ok(());
            }

            let exe_wide: Vec<u16> = format!("{}\0", exe_path_str).encode_utf16().collect();
            let byte_count = (exe_wide.len() * 2) as u32;

            let res = unsafe {
                RegSetValueExW(
                    hkey,
                    PCWSTR(value_name_wide.as_ptr()),
                    0,
                    REG_SZ,
                    Some(std::slice::from_raw_parts(
                        exe_wide.as_ptr() as *const u8,
                        byte_count as usize,
                    )),
                )
                .ok()
            };

            match res {
                Ok(()) => {
                    info!("Auto-start enabled: registry key set to {:?}", exe_path);
                    Ok(())
                }
                Err(e) => Err(anyhow::anyhow!(
                    "Failed to write auto-start registry value: {}",
                    e
                )),
            }
        } else {
            use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;

            let res = unsafe { RegDeleteValueW(hkey, PCWSTR(value_name_wide.as_ptr())) };

            // ERROR_FILE_NOT_FOUND means the key simply wasn't there — that's fine.
            if res == ERROR_FILE_NOT_FOUND || res.is_ok() {
                info!("Auto-start disabled (registry key removed or was not present)");
                Ok(())
            } else {
                Err(anyhow::anyhow!(
                    "Failed to delete auto-start registry value: {:?}",
                    res
                ))
            }
        };

        unsafe {
            let _ = RegCloseKey(hkey).ok();
        };
        result
    }

    #[cfg(not(windows))]
    {
        if enabled {
            warn!("Auto-start is not supported on this platform");
        }
        Ok(())
    }
}
