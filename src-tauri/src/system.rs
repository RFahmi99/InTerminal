//! Operating-system facts the UI adapts to.

use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETSCREENREADER, SystemParametersInfoW};

use crate::error::{AppError, CmdResult, ErrorCode};

/// Whether a screen reader (Narrator, NVDA, JAWS…) has announced itself to
/// Windows. The UI uses this to turn on the terminal's screen-reader mode by
/// default; the user can still override it.
#[tauri::command]
pub fn system_screen_reader_active() -> CmdResult<bool> {
    let mut active: i32 = 0;
    // SAFETY: SPI_GETSCREENREADER writes a BOOL to the pointer we pass.
    let ok = unsafe { SystemParametersInfoW(SPI_GETSCREENREADER, 0, (&raw mut active).cast(), 0) };
    if ok == 0 {
        return Err(AppError::new(
            ErrorCode::Io,
            format!("Windows didn't say whether a screen reader is running ({}).", std::io::Error::last_os_error()),
        ));
    }
    Ok(active != 0)
}
