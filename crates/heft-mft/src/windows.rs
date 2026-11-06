/// Win32 helpers for auto-mode: UAC elevation, save dialog, admin check.
///
/// All functions are #[cfg(windows)] — this module is only compiled on Windows.
/// On Linux, main.rs uses #[cfg(not(windows))] branches instead.

#[cfg(windows)]
use std::path::PathBuf;

#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::CloseHandle,
    Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
    System::Threading::{GetCurrentProcess, OpenProcessToken},
    UI::{
        Controls::Dialogs::{
            GetSaveFileNameW, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
        },
        Shell::ShellExecuteW,
    },
};

#[cfg(windows)]
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Check if the current process has Administrator privileges.
#[cfg(windows)]
pub fn is_elevated() -> bool {
    unsafe {
        let mut token = std::mem::zeroed();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut size = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

/// Re-launch this executable elevated via UAC (no args → re-enters auto mode).
/// Returns false if the user cancelled the UAC dialog.
#[cfg(windows)]
pub fn elevate_self() -> bool {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return false,
    };
    let exe_wide = to_wide(&exe.to_string_lossy());
    let verb = to_wide("runas");

    unsafe {
        let result = ShellExecuteW(
            0,                 // hwnd
            verb.as_ptr(),     // "runas" → UAC prompt
            exe_wide.as_ptr(), // executable path
            std::ptr::null(),  // no args → re-enters auto mode
            std::ptr::null(),  // working directory (inherit)
            1,                 // SW_SHOWNORMAL
        );
        (result as isize) > 32 // ShellExecuteW returns > 32 on success
    }
}

/// Show the native Explorer "Save As" dialog. Returns None if cancelled.
#[cfg(windows)]
pub fn save_file_dialog(default_name: &str) -> Option<PathBuf> {
    // File name buffer — pre-filled with the default, receives the result.
    let mut file_buf = [0u16; 260]; // MAX_PATH
    let default_wide: Vec<u16> = default_name.encode_utf16().collect();
    let copy_len = default_wide.len().min(file_buf.len() - 1);
    file_buf[..copy_len].copy_from_slice(&default_wide[..copy_len]);

    // Double-null-terminated filter: "description\0pattern\0...\0\0"
    let filter = to_wide("heft Files (*.heft)\0*.heft\0All Files (*.*)\0*.*\0");
    let def_ext = to_wide("heft");

    // Default to Desktop if available.
    let desktop = std::env::var("USERPROFILE")
        .map(|p| format!("{p}\\Desktop"))
        .unwrap_or_default();
    let initial_dir = to_wide(&desktop);

    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = file_buf.as_mut_ptr();
    ofn.nMaxFile = file_buf.len() as u32;
    ofn.lpstrDefExt = def_ext.as_ptr();
    ofn.Flags = OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST;
    if !desktop.is_empty() {
        ofn.lpstrInitialDir = initial_dir.as_ptr();
    }

    unsafe {
        if GetSaveFileNameW(&mut ofn) != 0 {
            let len = file_buf
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(file_buf.len());
            Some(PathBuf::from(String::from_utf16_lossy(&file_buf[..len])))
        } else {
            None
        }
    }
}

/// Keep the console window open after double-click.
#[cfg(windows)]
pub fn wait_for_keypress() {
    use std::io::Read;
    eprintln!("\nPress Enter to exit...");
    let _ = std::io::stdin().read(&mut [0u8]);
}
