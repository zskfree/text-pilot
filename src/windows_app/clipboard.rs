use std::thread;
use std::time::{Duration, Instant};
use windows::core::{Error, HRESULT};
use windows::Win32::Foundation::{
    GetLastError, GlobalFree, SetLastError, HANDLE, HGLOBAL, WIN32_ERROR,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    VK_CONTROL,
};

struct ClipboardGuard;

const CF_UNICODETEXT_VALUE: u32 = 13;
const CF_BITMAP_VALUE: u32 = 2;
const CF_METAFILEPICT_VALUE: u32 = 3;
const CF_PALETTE_VALUE: u32 = 9;
const CF_ENHMETAFILE_VALUE: u32 = 14;
const CF_OWNERDISPLAY_VALUE: u32 = 0x0080;
const CF_DSPBITMAP_VALUE: u32 = 0x0082;
const CF_DSPMETAFILEPICT_VALUE: u32 = 0x0083;
const CF_DSPENHMETAFILE_VALUE: u32 = 0x008e;
const CF_GDIOBJFIRST_VALUE: u32 = 0x0300;
const CF_GDIOBJLAST_VALUE: u32 = 0x03ff;
const COPY_TIMEOUT: Duration = Duration::from_millis(500);
const CLIPBOARD_OPEN_TIMEOUT: Duration = Duration::from_millis(500);

struct ClipboardFormatData {
    format: u32,
    bytes: Vec<u8>,
}

struct ClipboardSnapshot {
    formats: Vec<ClipboardFormatData>,
}

struct PreparedClipboardFormat {
    format: u32,
    memory: Option<HGLOBAL>,
}

impl Drop for PreparedClipboardFormat {
    fn drop(&mut self) {
        if let Some(memory) = self.memory.take() {
            unsafe {
                let _ = GlobalFree(Some(memory));
            }
        }
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

fn open_with_retry() -> Result<ClipboardGuard, Error> {
    let started = Instant::now();
    loop {
        if unsafe { OpenClipboard(None) }.is_ok() {
            return Ok(ClipboardGuard);
        }
        if started.elapsed() >= CLIPBOARD_OPEN_TIMEOUT {
            return Err(Error::from_thread());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

pub fn write_text(text: &str) -> Result<(), Error> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let byte_len = wide.len() * std::mem::size_of::<u16>();
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, byte_len) }?;
    let pointer = unsafe { GlobalLock(memory) };
    if pointer.is_null() {
        unsafe {
            let _ = GlobalFree(Some(memory));
        }
        return Err(Error::from_thread());
    }
    unsafe {
        std::ptr::copy_nonoverlapping(wide.as_ptr().cast::<u8>(), pointer.cast::<u8>(), byte_len);
        let _ = GlobalUnlock(memory);
    }

    let _guard = match open_with_retry() {
        Ok(guard) => guard,
        Err(error) => {
            unsafe {
                let _ = GlobalFree(Some(memory));
            }
            return Err(error);
        }
    };
    unsafe { EmptyClipboard()? };
    if unsafe { SetClipboardData(CF_UNICODETEXT_VALUE, Some(HANDLE(memory.0))) }.is_err() {
        unsafe {
            let _ = GlobalFree(Some(memory));
        }
        return Err(Error::from_thread());
    }
    // Ownership of memory transfers to the system after SetClipboardData succeeds.
    Ok(())
}

pub fn read_selected_text_compatibility() -> Result<Option<String>, Error> {
    with_restored_snapshot(
        snapshot_clipboard,
        copy_selection_and_read,
        restore_clipboard,
    )
}

fn snapshot_clipboard() -> Result<ClipboardSnapshot, Error> {
    let _guard = open_with_retry()?;
    let mut formats = Vec::new();
    let mut format = 0;
    loop {
        unsafe {
            SetLastError(WIN32_ERROR(0));
        }
        format = unsafe { EnumClipboardFormats(format) };
        if format == 0 {
            check_clipboard_enumeration_end(unsafe { GetLastError() })?;
            break;
        }
        if !can_snapshot_as_hglobal(format) {
            return Err(unsupported_clipboard_format(format));
        }
        let handle = unsafe { GetClipboardData(format) }?;
        formats.push(ClipboardFormatData {
            format,
            bytes: copy_hglobal_bytes(HGLOBAL(handle.0), format)?,
        });
    }
    Ok(ClipboardSnapshot { formats })
}

fn check_clipboard_enumeration_end(error: WIN32_ERROR) -> Result<(), Error> {
    if error.0 != 0 {
        error.ok()?;
    }
    Ok(())
}

fn restore_clipboard(snapshot: ClipboardSnapshot) -> Result<(), Error> {
    let mut prepared = snapshot
        .formats
        .into_iter()
        .map(|data| {
            allocate_clipboard_bytes(&data.bytes).map(|memory| PreparedClipboardFormat {
                format: data.format,
                memory: Some(memory),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let _guard = open_with_retry()?;
    unsafe { EmptyClipboard()? };
    for data in &mut prepared {
        let memory = data.memory.take().ok_or_else(Error::from_thread)?;
        if let Err(error) = unsafe { SetClipboardData(data.format, Some(HANDLE(memory.0))) } {
            data.memory = Some(memory);
            return Err(error);
        }
    }
    Ok(())
}

fn can_snapshot_as_hglobal(format: u32) -> bool {
    !matches!(
        format,
        CF_BITMAP_VALUE
            | CF_METAFILEPICT_VALUE
            | CF_PALETTE_VALUE
            | CF_ENHMETAFILE_VALUE
            | CF_OWNERDISPLAY_VALUE
            | CF_DSPBITMAP_VALUE
            | CF_DSPMETAFILEPICT_VALUE
            | CF_DSPENHMETAFILE_VALUE
    ) && !(CF_GDIOBJFIRST_VALUE..=CF_GDIOBJLAST_VALUE).contains(&format)
}

fn unsupported_clipboard_format(format: u32) -> Error {
    Error::new(
        HRESULT(0x80004005_u32 as i32),
        format!("剪贴板包含无法安全备份的格式 0x{format:04X}，已取消兼容模式读取"),
    )
}

fn copy_hglobal_bytes(memory: HGLOBAL, format: u32) -> Result<Vec<u8>, Error> {
    let byte_len = unsafe { GlobalSize(memory) };
    if byte_len == 0 {
        return Err(unsupported_clipboard_format(format));
    }
    let pointer = unsafe { GlobalLock(memory) };
    if pointer.is_null() {
        return Err(Error::from_thread());
    }
    let bytes = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), byte_len) }.to_vec();
    unsafe {
        let _ = GlobalUnlock(memory);
    }
    Ok(bytes)
}

fn allocate_clipboard_bytes(bytes: &[u8]) -> Result<HGLOBAL, Error> {
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) }?;
    let pointer = unsafe { GlobalLock(memory) };
    if pointer.is_null() {
        unsafe {
            let _ = GlobalFree(Some(memory));
        }
        return Err(Error::from_thread());
    }
    if !bytes.is_empty() {
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast::<u8>(), bytes.len());
        }
    }
    unsafe {
        let _ = GlobalUnlock(memory);
    }
    Ok(memory)
}

fn copy_selection_and_read() -> Result<Option<String>, Error> {
    let previous_sequence = unsafe { GetClipboardSequenceNumber() };
    send_ctrl_c()?;

    let started = Instant::now();
    loop {
        let sequence_changed = unsafe { GetClipboardSequenceNumber() } != previous_sequence;
        if sequence_changed && unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT_VALUE) }.is_ok() {
            if let Some(text) = read_text()? {
                return Ok(Some(text));
            }
        }
        if started.elapsed() >= COPY_TIMEOUT {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn read_text() -> Result<Option<String>, Error> {
    let _guard = open_with_retry()?;
    let handle = match unsafe { GetClipboardData(CF_UNICODETEXT_VALUE) } {
        Ok(handle) => handle,
        Err(_) => return Ok(None),
    };
    let memory = HGLOBAL(handle.0);
    let pointer = unsafe { GlobalLock(memory) };
    if pointer.is_null() {
        return Err(Error::from_thread());
    }

    let byte_len = unsafe { GlobalSize(memory) };
    let units = unsafe {
        std::slice::from_raw_parts(pointer.cast::<u16>(), byte_len / std::mem::size_of::<u16>())
    };
    let length = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    let text = String::from_utf16_lossy(&units[..length]);
    unsafe {
        let _ = GlobalUnlock(memory);
    }
    Ok((!text.trim().is_empty()).then_some(text))
}

fn send_ctrl_c() -> Result<(), Error> {
    let ctrl_is_down = unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) } < 0;
    if ctrl_is_down {
        send_inputs(&[
            keyboard_input(b'C' as u16, Default::default()),
            keyboard_input(b'C' as u16, KEYEVENTF_KEYUP),
        ])
    } else {
        send_inputs(&[
            keyboard_input(VK_CONTROL.0, Default::default()),
            keyboard_input(b'C' as u16, Default::default()),
            keyboard_input(b'C' as u16, KEYEVENTF_KEYUP),
            keyboard_input(VK_CONTROL.0, KEYEVENTF_KEYUP),
        ])
    }
}

pub fn replay_ctrl_key(virtual_key: u32, count: u8) -> Result<(), Error> {
    let ctrl_is_down = unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) } < 0;
    send_inputs(&ctrl_key_inputs(virtual_key, count, ctrl_is_down))
}

fn ctrl_key_inputs(virtual_key: u32, count: u8, ctrl_is_down: bool) -> Vec<INPUT> {
    let inputs_per_tap = if ctrl_is_down { 2 } else { 4 };
    let mut inputs = Vec::with_capacity(count as usize * inputs_per_tap);
    for _ in 0..count {
        if !ctrl_is_down {
            inputs.push(keyboard_input(VK_CONTROL.0, Default::default()));
        }
        inputs.extend([
            keyboard_input(virtual_key as u16, Default::default()),
            keyboard_input(virtual_key as u16, KEYEVENTF_KEYUP),
        ]);
        if !ctrl_is_down {
            inputs.push(keyboard_input(VK_CONTROL.0, KEYEVENTF_KEYUP));
        }
    }
    inputs
}

fn send_inputs(inputs: &[INPUT]) -> Result<(), Error> {
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        return Err(Error::from_thread());
    }
    Ok(())
}

fn keyboard_input(
    key: u16,
    flags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS,
) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(key),
                dwFlags: flags,
                ..Default::default()
            },
        },
    }
}

fn with_restored_snapshot<Snapshot, Value, Failure, Capture, Operation, Restore>(
    capture: Capture,
    operation: Operation,
    restore: Restore,
) -> Result<Value, Failure>
where
    Capture: FnOnce() -> Result<Snapshot, Failure>,
    Operation: FnOnce() -> Result<Value, Failure>,
    Restore: FnOnce(Snapshot) -> Result<(), Failure>,
{
    let snapshot = capture()?;
    let result = operation();
    restore(snapshot)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_owned_clipboard_formats_are_rejected_before_compatibility_copy() {
        for format in [
            CF_BITMAP_VALUE,
            CF_METAFILEPICT_VALUE,
            CF_PALETTE_VALUE,
            CF_ENHMETAFILE_VALUE,
            CF_OWNERDISPLAY_VALUE,
            CF_DSPBITMAP_VALUE,
            CF_DSPMETAFILEPICT_VALUE,
            CF_DSPENHMETAFILE_VALUE,
        ] {
            assert!(!can_snapshot_as_hglobal(format));
        }
        assert!(can_snapshot_as_hglobal(CF_UNICODETEXT_VALUE));
    }

    #[test]
    fn clipboard_enumeration_distinguishes_end_of_data_from_failure() {
        assert!(check_clipboard_enumeration_end(WIN32_ERROR(0)).is_ok());
        assert!(check_clipboard_enumeration_end(WIN32_ERROR(5)).is_err());
    }

    #[test]
    fn compatibility_read_restores_clipboard_after_success() {
        let mut restored = None;

        let result = with_restored_snapshot(
            || Ok::<_, &'static str>("原剪贴板"),
            || Ok::<_, &'static str>("选中文本"),
            |snapshot| {
                restored = Some(snapshot);
                Ok::<_, &'static str>(())
            },
        )
        .unwrap();

        assert_eq!(result, "选中文本");
        assert_eq!(restored, Some("原剪贴板"));
    }

    #[test]
    fn compatibility_read_restores_clipboard_after_failure() {
        let mut restored = None;

        let result = with_restored_snapshot(
            || Ok::<_, &'static str>("原剪贴板"),
            || Err::<&'static str, _>("复制失败"),
            |snapshot| {
                restored = Some(snapshot);
                Ok::<_, &'static str>(())
            },
        );

        assert_eq!(result, Err("复制失败"));
        assert_eq!(restored, Some("原剪贴板"));
    }

    #[test]
    fn compatibility_read_reports_restore_failure() {
        let result = with_restored_snapshot(
            || Ok::<_, &'static str>("原剪贴板"),
            || Err::<&'static str, _>("复制失败"),
            |_| Err::<(), _>("恢复失败"),
        );

        assert_eq!(result, Err("恢复失败"));
    }

    #[test]
    fn compatibility_failure_does_not_poison_the_next_attempt() {
        let first = with_restored_snapshot(
            || Ok::<_, &'static str>("original"),
            || Err::<(), _>("copy failed"),
            |_| Ok::<_, &'static str>(()),
        );
        let second = with_restored_snapshot(
            || Ok::<_, &'static str>("original"),
            || Ok::<_, &'static str>("selected"),
            |_| Ok::<_, &'static str>(()),
        );

        assert_eq!(first, Err("copy failed"));
        assert_eq!(second, Ok("selected"));
    }

    #[test]
    fn replayed_ctrl_f8_releases_both_keys() {
        let inputs = ctrl_key_inputs(0x77, 1, false);

        assert_eq!(inputs.len(), 4);
        let keys = inputs
            .iter()
            .map(|input| unsafe { input.Anonymous.ki })
            .map(|input| (input.wVk.0, input.dwFlags))
            .collect::<Vec<_>>();
        assert_eq!(keys[0], (VK_CONTROL.0, Default::default()));
        assert_eq!(keys[1], (0x77, Default::default()));
        assert_eq!(keys[2], (0x77, KEYEVENTF_KEYUP));
        assert_eq!(keys[3], (VK_CONTROL.0, KEYEVENTF_KEYUP));
    }

    #[test]
    fn replay_does_not_release_physically_held_ctrl() {
        let inputs = ctrl_key_inputs(0x77, 1, true);
        let keys = inputs
            .iter()
            .map(|input| unsafe { input.Anonymous.ki })
            .map(|input| (input.wVk.0, input.dwFlags))
            .collect::<Vec<_>>();

        assert_eq!(
            keys,
            vec![(0x77, Default::default()), (0x77, KEYEVENTF_KEYUP)]
        );
    }
}
