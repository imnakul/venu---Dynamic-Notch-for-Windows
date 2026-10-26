//! A single, small wrapper around the common "Open File" dialog.
//!
//! `GetOpenFileNameW` is used rather than the newer `IFileOpenDialog` because
//! it needs no COM apartment on the calling thread — and the settings window
//! runs on eframe's thread, whose apartment model is not ours to assume.

use windows::core::PCWSTR;
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_ALLOWMULTISELECT, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR,
    OFN_NODEREFERENCELINKS, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Ask the user for an image. `title` names the dialog, so the flash picker
/// and the wallpaper picker can each say what they are for. Returns `None` if
/// they cancel.
pub fn pick_image(initial: &str, title: &str) -> Option<String> {
    // Null-separated, double-null-terminated pairs, as the API wants them.
    // Built here rather than as a const so the escaping stays readable.
    // Every image extension users actually meet, not only the ones WIC is
    // guaranteed to decode: the renderer degrades gracefully when a picked
    // file turns out not to load, but the dialog should never be the thing
    // that hides a picture from the user.
    //
    // The label is only display text — the pattern is what filters — so it
    // spells the formats out (common ones first, since a narrow dialog box
    // truncates the tail) while the pattern quietly accepts everything,
    // including the .jfif the label skips.
    let mut filter: Vec<u16> = Vec::new();
    for part in [
        "Images (*.jpg, *.jpeg, *.png, *.gif, *.webp, *.bmp, *.tif, *.tiff, *.ico, *.avif, *.heic, *.heif, *.svg)",
        "*.png;*.jpg;*.jpeg;*.jfif;*.bmp;*.gif;*.webp;*.tif;*.tiff;*.ico;*.avif;*.heic;*.heif;*.svg",
        "All files (*.*)",
        "*.*",
    ] {
        filter.extend(part.encode_utf16());
        filter.push(0);
    }
    filter.push(0);

    // The dialog writes the chosen path back into this buffer, so it has to be
    // big enough for a long path and owned for the duration of the call.
    let mut buffer = vec![0u16; 1024];
    let trimmed = initial.trim();
    if !trimmed.is_empty() && trimmed.len() < 1000 {
        let src = wide(trimmed);
        buffer[..src.len()].copy_from_slice(&src);
    }

    let title = wide(title);

    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        nFilterIndex: 1,
        lpstrFile: windows::core::PWSTR(buffer.as_mut_ptr()),
        nMaxFile: buffer.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        // NOCHANGEDIR matters: without it the dialog moves the whole process's
        // working directory, which would break every relative path we hold.
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };

    let ok = unsafe { GetOpenFileNameW(&mut ofn) };
    if !ok.as_bool() {
        return None;
    }

    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    let path = String::from_utf16_lossy(&buffer[..end]);
    if path.trim().is_empty() {
        None
    } else {
        Some(path)
    }
}

/// Ask for any number of files, apps, shortcuts or scripts. Returns the full
/// paths picked, or nothing if cancelled.
pub fn pick_files(title: &str) -> Vec<String> {
    let mut filter: Vec<u16> = Vec::new();
    for part in ["All files (*.*)", "*.*"] {
        filter.extend(part.encode_utf16());
        filter.push(0);
    }
    filter.push(0);

    // Room for a good number of long paths: a multi-select fills this with
    // the folder, then each file name.
    let mut buffer = vec![0u16; 32 * 1024];
    let title = wide(title);
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        nFilterIndex: 1,
        lpstrFile: windows::core::PWSTR(buffer.as_mut_ptr()),
        nMaxFile: buffer.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_FILEMUSTEXIST
            | OFN_PATHMUSTEXIST
            | OFN_NOCHANGEDIR
            | OFN_ALLOWMULTISELECT
            | OFN_EXPLORER
            // Pin the shortcut itself, not whatever it points at.
            | OFN_NODEREFERENCELINKS,
        ..Default::default()
    };
    if !unsafe { GetOpenFileNameW(&mut ofn) }.as_bool() {
        return Vec::new();
    }
    split_multi(&buffer)
}

/// Read a multi-select result: one full path, or a folder followed by file
/// names, each null-terminated, the list ending in an empty string.
fn split_multi(buffer: &[u16]) -> Vec<String> {
    let parts: Vec<String> = buffer
        .split(|&c| c == 0)
        .take_while(|part| !part.is_empty())
        .map(String::from_utf16_lossy)
        .collect();
    match parts.as_slice() {
        [] => Vec::new(),
        [single] => vec![single.clone()],
        [folder, files @ ..] => files
            .iter()
            .map(|file| {
                let mut path = std::path::PathBuf::from(folder);
                path.push(file);
                path.to_string_lossy().into_owned()
            })
            .collect(),
    }
}

/// Ask for a folder. Uses the modern folder picker, on a thread of its own
/// with its own COM apartment, so nothing about eframe's thread is assumed.
pub fn pick_folder(title: &str) -> Option<String> {
    let title = title.to_owned();
    std::thread::spawn(move || unsafe {
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED,
        };
        use windows::Win32::UI::Shell::{
            FileOpenDialog, IFileOpenDialog, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS,
            SIGDN_FILESYSPATH,
        };
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let picked = (|| {
            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            let options = dialog.GetOptions().ok()?;
            dialog
                .SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)
                .ok()?;
            let wide_title = wide(&title);
            let _ = dialog.SetTitle(PCWSTR(wide_title.as_ptr()));
            dialog.Show(None).ok()?;
            let item = dialog.GetResult().ok()?;
            let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
            let path = name.to_string().ok();
            CoTaskMemFree(Some(name.0 as *const std::ffi::c_void));
            path
        })();
        CoUninitialize();
        picked
    })
    .join()
    .ok()
    .flatten()
    .filter(|path| !path.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::split_multi;

    fn buf(parts: &[&str]) -> Vec<u16> {
        let mut out = Vec::new();
        for part in parts {
            out.extend(part.encode_utf16());
            out.push(0);
        }
        out.push(0);
        out
    }

    #[test]
    fn single_and_multiple_picks_both_come_back_as_full_paths() {
        assert_eq!(split_multi(&buf(&[r"C:\a\b.txt"])), vec![r"C:\a\b.txt"]);
        let many = split_multi(&buf(&[r"C:\dir", "one.lnk", "two.bat"]));
        assert_eq!(many.len(), 2);
        assert!(many[0].ends_with("one.lnk") && many[1].ends_with("two.bat"));
    }
}
