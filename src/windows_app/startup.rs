use std::path::Path;
use windows::core::{w, Error, HSTRING, PCWSTR};
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
};

const VALUE_NAME: PCWSTR = w!("TextPilot");
// Removed on every update so upgrades do not leave the pre-v0.5 startup entry behind.
const LEGACY_VALUE_NAME: PCWSTR = w!("PromptOptimizer");

pub fn set_auto_start(enabled: bool, exe_path: &Path) -> Result<(), Error> {
    let mut key = HKEY::default();
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
        .ok()?;
    }

    let result = if enabled {
        let command = HSTRING::from(format!("\"{}\"", exe_path.display()));
        let bytes = unsafe {
            std::slice::from_raw_parts(
                command.as_ptr().cast::<u8>(),
                (command.len() + 1) * std::mem::size_of::<u16>(),
            )
        };
        unsafe { RegSetValueExW(key, VALUE_NAME, None, REG_SZ, Some(bytes)).ok() }
    } else {
        delete_value_if_present(key, VALUE_NAME)
    };

    let result = result.and_then(|_| delete_value_if_present(key, LEGACY_VALUE_NAME));

    unsafe {
        let _ = RegCloseKey(key);
    }
    result
}

fn delete_value_if_present(key: HKEY, name: PCWSTR) -> Result<(), Error> {
    let code = unsafe { RegDeleteValueW(key, name) };
    if code == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        code.ok()
    }
}
