//! The PATH a newly started program would get. VCE's own environment is
//! frozen at launch, so programs installed afterwards (e.g. with winget) would
//! stay invisible to new shells and to command search until VCE restarted.

use std::ffi::OsString;

use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};

const MACHINE_ENVIRONMENT: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";
const USER_ENVIRONMENT: &str = "Environment";

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// Reads a string value, expanding `%VAR%` references in REG_EXPAND_SZ values.
fn read_string(root: HKEY, subkey_name: &str, value_name: &str) -> Option<String> {
    let (subkey, value) = (wide(subkey_name), wide(value_name));
    let mut bytes: u32 = 0;
    // SAFETY: null data pointer with a valid size pointer queries the size.
    let status = unsafe {
        RegGetValueW(root, subkey.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut bytes)
    };
    if status != 0 {
        return None;
    }
    let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
    // SAFETY: `buffer` holds `bytes` bytes, as the size query reported.
    let status = unsafe {
        RegGetValueW(
            root,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != 0 {
        log::warn!("Could not read registry value {subkey_name}\\{value_name}: error {status}");
        return None;
    }
    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..len]))
}

/// The machine PATH followed by the user PATH, as Windows composes it for new
/// processes. Falls back to this process's PATH if the registry can't be read.
pub fn current() -> OsString {
    let machine = read_string(HKEY_LOCAL_MACHINE, MACHINE_ENVIRONMENT, "Path");
    let user = read_string(HKEY_CURRENT_USER, USER_ENVIRONMENT, "Path");
    let parts: Vec<String> = [machine, user].into_iter().flatten().filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        log::warn!("Could not read PATH from the registry; using VCE's own PATH");
        return std::env::var_os("PATH").unwrap_or_default();
    }
    OsString::from(parts.join(";"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_path_is_expanded_and_includes_system32() {
        let path = current().to_string_lossy().to_lowercase();
        assert!(path.contains(r"\system32"), "PATH was {path}");
        assert!(!path.contains("%systemroot%"), "REG_EXPAND_SZ wasn't expanded: {path}");
    }
}
