//! WASAPI device information helpers
//!
//! Provides functions for logging audio device names and enumerating endpoints.

use tracing::{info, warn};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    eCapture, eRender, IMMDevice, IMMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToBSTR;
use windows::Win32::System::Com::STGM;

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
