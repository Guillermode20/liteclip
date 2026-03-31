//! WASAPI device information helpers
//!
//! Provides functions for logging audio device names and enumerating endpoints.

use tracing::{info, warn};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    eCapture, eRender, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToBSTR;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED, STGM,
};

/// Get the friendly name of a WASAPI audio device.
///
/// Returns `None` if the name cannot be retrieved (non-fatal).
pub fn get_device_friendly_name(device: &IMMDevice) -> Option<String> {
    unsafe {
        let store = device.OpenPropertyStore(STGM(0)).ok()?; // STGM_READ = 0
        let prop = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
        let bstr = PropVariantToBSTR(&prop).ok()?;
        Some(bstr.to_string())
    }
}

/// Get the endpoint ID of a WASAPI audio device.
pub fn get_device_id(device: &IMMDevice) -> Option<String> {
    unsafe {
        let id = device.GetId().ok()?;
        let s = id.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(id.0 as *const _));
        s
    }
}

/// Log a single device with its friendly name and endpoint ID.
pub fn log_device(label: &str, device: &IMMDevice) {
    let name = get_device_friendly_name(device).unwrap_or_else(|| "<unknown>".to_string());
    let id = get_device_id(device).unwrap_or_else(|| "<no id>".to_string());
    info!("{}: \"{}\" (endpoint: {})", label, name, id);
}

/// Enumerate and log all active audio capture (microphone) devices.
pub fn log_all_capture_devices(enumerator: &IMMDeviceEnumerator) {
    match unsafe { enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) } {
        Ok(collection) => {
            let count = unsafe { collection.GetCount() }.unwrap_or(0);
            info!("Available capture (microphone) devices: {}", count);

            for i in 0..count {
                if let Ok(device) = unsafe { collection.Item(i) } {
                    let name = get_device_friendly_name(&device)
                        .unwrap_or_else(|| "<unknown>".to_string());
                    let id = get_device_id(&device).unwrap_or_else(|| "<no id>".to_string());
                    info!("  [{}] \"{}\" (endpoint: {})", i, name, id);
                }
            }

            if count == 0 {
                warn!("No active capture devices found — microphone audio will not be available");
            }
        }
        Err(e) => {
            warn!("Failed to enumerate capture devices: {}", e);
        }
    }
}

/// List active capture devices as `(display_name, endpoint_id)` tuples.
///
/// Always includes a synthetic default option as the first entry:
/// `("System Default", "default")`.
pub fn list_capture_devices() -> Vec<(String, String)> {
    let mut devices = vec![("System Default".to_string(), "default".to_string())];

    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr.is_err() {
        warn!(
            "Failed to initialize COM while listing capture devices: {:?}",
            hr
        );
        return devices;
    }
    let initialized_by_us = hr.is_ok();

    let result = (|| -> windows::core::Result<()> {
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }?;
        let collection = unsafe { enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) }?;
        let count = unsafe { collection.GetCount() }?;

        for i in 0..count {
            let device = unsafe { collection.Item(i) }?;
            let name = get_device_friendly_name(&device).unwrap_or_else(|| "<unknown>".to_string());
            let id = get_device_id(&device).unwrap_or_else(|| "<no id>".to_string());

            if id != "<no id>" {
                devices.push((name, id));
            }
        }

        Ok(())
    })();

    if let Err(e) = result {
        warn!("Failed to list capture devices: {}", e);
    }

    if initialized_by_us {
        unsafe {
            CoUninitialize();
        }
    }

    devices
}

/// Enumerate and log all active audio render (output/loopback) devices.
pub fn log_all_render_devices(enumerator: &IMMDeviceEnumerator) {
    match unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) } {
        Ok(collection) => {
            let count = unsafe { collection.GetCount() }.unwrap_or(0);
            info!("Available render (output) devices: {}", count);

            for i in 0..count {
                if let Ok(device) = unsafe { collection.Item(i) } {
                    let name = get_device_friendly_name(&device)
                        .unwrap_or_else(|| "<unknown>".to_string());
                    let id = get_device_id(&device).unwrap_or_else(|| "<no id>".to_string());
                    info!("  [{}] \"{}\" (endpoint: {})", i, name, id);
                }
            }
        }
        Err(e) => {
            warn!("Failed to enumerate render devices: {}", e);
        }
    }
}
