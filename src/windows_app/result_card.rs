use super::clipboard;
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::sync::Mutex;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    COLORREF, HINSTANCE, HMENU, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreatePen, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint,
    FillRect, GetMonitorInfoW, MonitorFromRect, RoundRect, SelectObject, SetBkColor, SetBkMode,
    SetTextColor, DEFAULT_CHARSET, DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE, DT_VCENTER,
    FF_DONTCARE, FONT_CLIP_PRECISION, FONT_OUTPUT_PRECISION, FONT_QUALITY, HBRUSH, HDC, HFONT,
    HPEN, MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT, PS_SOLID, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, REG_DWORD,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_ESCAPE, VK_RETURN};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetCursorPos,
    GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, IsWindow, LoadCursorW, PostMessageW,
    RegisterClassW, SendMessageW, SetFocus, SetForegroundWindow, SetWindowLongPtrW, ShowWindow,
    CREATESTRUCTW, EM_SETSEL, ES_AUTOVSCROLL, ES_MULTILINE, ES_WANTRETURN, GWLP_USERDATA,
    GWLP_WNDPROC, IDC_ARROW, SW_SHOW, WA_INACTIVE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_ACTIVATE,
    WM_APP, WM_COMMAND, WM_CREATE, WM_CTLCOLORBTN, WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC, WM_DESTROY,
    WM_ERASEBKGND, WM_KEYDOWN, WM_NCCREATE, WM_PAINT, WM_SETFONT, WNDCLASSW, WNDPROC, WS_CHILD,
    WS_CLIPCHILDREN, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_TABSTOP, WS_VISIBLE,
    WS_VSCROLL,
};

pub const WM_RESULT_CARD_CLOSED: u32 = WM_APP + 30;
pub const WM_RETRY_ACTION: u32 = WM_APP + 31;

const CLASS_NAME: PCWSTR = w!("TextPilot.NativeResultCard");
const DEFAULT_CARD_WIDTH: i32 = 520;
const DEFAULT_CARD_HEIGHT: i32 = 320;

const IDC_EDIT: usize = 1001;
const IDC_BTN_RETRY: usize = 1002;
const IDC_BTN_COPY: usize = 1003;
const IDC_BTN_CLOSE: usize = 1004;

static ACTIVE_RESULT_CARD_HWND: Mutex<Option<isize>> = Mutex::new(None);

#[derive(Clone, Debug)]
pub struct ResultCardData {
    pub language: String,
    pub action_id: String,
    pub action_name: String,
    pub model: String,
    pub original_text: String,
    pub result_text: String,
}

struct CreateParams {
    owner: HWND,
    data: ResultCardData,
}

struct CardThemeColors {
    is_dark: bool,
    bg_shell: COLORREF,
    bg_bar: COLORREF,
    border_bar: COLORREF,
    bg_edit: COLORREF,
    text_primary: COLORREF,
    text_secondary: COLORREF,
    text_tertiary: COLORREF,
    text_mono: COLORREF,
    badge_success_bg: COLORREF,
    badge_success_border: COLORREF,
    badge_success_text: COLORREF,
    brush_shell: HBRUSH,
    brush_bar: HBRUSH,
    brush_edit: HBRUSH,
    brush_badge_success: HBRUSH,
    pen_border: HPEN,
    pen_badge_success: HPEN,
}

impl CardThemeColors {
    fn new(is_dark: bool) -> Self {
        if is_dark {
            let bg_shell = COLORREF(0x001B_1818);
            let bg_bar = COLORREF(0x0023_2020);
            let border_bar = COLORREF(0x0040_3533);
            let bg_edit = COLORREF(0x0014_1212);
            let text_primary = COLORREF(0x00F5_F4F4);
            let text_secondary = COLORREF(0x00D8_D4D4);
            let text_tertiary = COLORREF(0x008A_807A);
            let text_mono = COLORREF(0x00E0_DCDA);
            let badge_success_bg = COLORREF(0x0028_4020);
            let badge_success_border = COLORREF(0x003A_652D);
            let badge_success_text = COLORREF(0x0080_DE4A);

            unsafe {
                Self {
                    is_dark: true,
                    bg_shell,
                    bg_bar,
                    border_bar,
                    bg_edit,
                    text_primary,
                    text_secondary,
                    text_tertiary,
                    text_mono,
                    badge_success_bg,
                    badge_success_border,
                    badge_success_text,
                    brush_shell: CreateSolidBrush(bg_shell),
                    brush_bar: CreateSolidBrush(bg_bar),
                    brush_edit: CreateSolidBrush(bg_edit),
                    brush_badge_success: CreateSolidBrush(badge_success_bg),
                    pen_border: CreatePen(PS_SOLID, 1, border_bar),
                    pen_badge_success: CreatePen(PS_SOLID, 1, badge_success_border),
                }
            }
        } else {
            let bg_shell = COLORREF(0x00FF_FFFF);
            let bg_bar = COLORREF(0x00FC_FAF8);
            let border_bar = COLORREF(0x00F0_E8E2);
            let bg_edit = COLORREF(0x00FA_F7F5);
            let text_primary = COLORREF(0x002A_170F);
            let text_secondary = COLORREF(0x0069_5547);
            let text_tertiary = COLORREF(0x0094_8070);
            let text_mono = COLORREF(0x002A_170F);
            let badge_success_bg = COLORREF(0x00F4_FDF0);
            let badge_success_border = COLORREF(0x00D0_F7BB);
            let badge_success_text = COLORREF(0x003D_8015);

            unsafe {
                Self {
                    is_dark: false,
                    bg_shell,
                    bg_bar,
                    border_bar,
                    bg_edit,
                    text_primary,
                    text_secondary,
                    text_tertiary,
                    text_mono,
                    badge_success_bg,
                    badge_success_border,
                    badge_success_text,
                    brush_shell: CreateSolidBrush(bg_shell),
                    brush_bar: CreateSolidBrush(bg_bar),
                    brush_edit: CreateSolidBrush(bg_edit),
                    brush_badge_success: CreateSolidBrush(badge_success_bg),
                    pen_border: CreatePen(PS_SOLID, 1, border_bar),
                    pen_badge_success: CreatePen(PS_SOLID, 1, badge_success_border),
                }
            }
        }
    }

    fn release(&self) {
        unsafe {
            let _ = DeleteObject(self.brush_shell.into());
            let _ = DeleteObject(self.brush_bar.into());
            let _ = DeleteObject(self.brush_edit.into());
            let _ = DeleteObject(self.brush_badge_success.into());
            let _ = DeleteObject(self.pen_border.into());
            let _ = DeleteObject(self.pen_badge_success.into());
        }
    }
}

struct NativeCardState {
    owner: HWND,
    data: ResultCardData,
    edit_hwnd: HWND,
    btn_retry_hwnd: HWND,
    btn_copy_hwnd: HWND,
    btn_close_hwnd: HWND,
    orig_edit_proc: WNDPROC,
    theme: CardThemeColors,
    font_ui_bold: HFONT,
    font_ui_normal: HFONT,
    font_ui_small: HFONT,
    font_mono: HFONT,
    activated_once: bool,
}

pub fn show_result_card(owner: HWND, data: ResultCardData) {
    close_existing_card();

    let module = match unsafe { GetModuleHandleW(None) } {
        Ok(handle) => handle,
        Err(_) => return,
    };

    let class = WNDCLASSW {
        lpfnWndProc: Some(native_card_window_proc),
        hInstance: HINSTANCE(module.0),
        lpszClassName: CLASS_NAME,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() },
        ..Default::default()
    };
    let _ = unsafe { RegisterClassW(&class) };

    let dpi = 96;
    let (initial_x, initial_y, width, height) = calculate_card_bounds(dpi);

    let params = Box::new(CreateParams { owner, data });
    let params_ptr = Box::into_raw(params);

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0),
            CLASS_NAME,
            w!("TextPilot Result"),
            WINDOW_STYLE(WS_POPUP.0 | WS_CLIPCHILDREN.0),
            initial_x,
            initial_y,
            width,
            height,
            None,
            None,
            Some(HINSTANCE(module.0)),
            Some(params_ptr as *const c_void),
        )
    };

    let Ok(hwnd) = hwnd else {
        return;
    };

    // 应用 Windows 11 DWM 优雅圆角
    let preference = DWMWCP_ROUND;
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            std::ptr::from_ref(&preference).cast(),
            std::mem::size_of_val(&preference) as u32,
        )
    };

    if let Ok(mut lock) = ACTIVE_RESULT_CARD_HWND.lock() {
        *lock = Some(hwnd.0 as isize);
    }

    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
}

pub fn close_existing_card() {
    if let Ok(mut lock) = ACTIVE_RESULT_CARD_HWND.lock() {
        if let Some(hwnd_raw) = lock.take() {
            let hwnd = HWND(hwnd_raw as *mut c_void);
            if unsafe { IsWindow(Some(hwnd)) }.as_bool() {
                let _ = unsafe { DestroyWindow(hwnd) };
            }
        }
    }
}

fn calculate_card_bounds(dpi: u32) -> (i32, i32, i32, i32) {
    let width = scale(DEFAULT_CARD_WIDTH, dpi);
    let height = scale(DEFAULT_CARD_HEIGHT, dpi);
    let gap = scale(10, dpi);

    let anchor = super::selection::take_selection_rect().unwrap_or_else(|| {
        let mut point = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut point);
        }
        RECT {
            left: point.x,
            top: point.y,
            right: point.x + 1,
            bottom: point.y + 1,
        }
    });

    let monitor = unsafe { MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST) };
    let mut monitor_info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let work = if unsafe { GetMonitorInfoW(monitor, &mut monitor_info) }.as_bool() {
        monitor_info.rcWork
    } else {
        RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        }
    };

    let mut x = anchor.left;
    let mut y = anchor.bottom + gap;

    if x + width > work.right {
        x = work.right - width - gap;
    }
    if x < work.left + gap {
        x = work.left + gap;
    }

    if y + height > work.bottom {
        y = anchor.top - height - gap;
    }
    if y < work.top + gap {
        y = work.top + gap;
    }

    (x, y, width, height)
}

fn scale(value: i32, dpi: u32) -> i32 {
    let dpi = dpi.max(96);
    ((value as i64 * dpi as i64 + 48) / 96) as i32
}

fn wide(value: &str) -> Vec<u16> {
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn is_system_dark_mode() -> bool {
    let mut key = HKEY::default();
    let subkey = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    if unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            0,
            KEY_READ,
            &mut key,
        )
    }
    .is_ok()
    {
        let mut data: u32 = 1;
        let mut size = std::mem::size_of::<u32>() as u32;
        let mut reg_type = REG_DWORD;
        let val_name = wide("AppsUseLightTheme");
        let query_res = unsafe {
            RegQueryValueExW(
                key,
                PCWSTR(val_name.as_ptr()),
                None,
                Some(&mut reg_type),
                Some(&mut data as *mut u32 as *mut u8),
                Some(&mut size),
            )
        };
        let _ = unsafe { RegCloseKey(key) };
        if query_res.is_ok() {
            return data == 0;
        }
    }
    false
}

unsafe fn create_gdi_font(name: &str, height_pt: i32, weight: i32, dpi: u32) -> HFONT {
    let wide_name = wide(name);
    CreateFontW(
        -scale(height_pt, dpi),
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET,
        FONT_OUTPUT_PRECISION::default(),
        FONT_CLIP_PRECISION::default(),
        FONT_QUALITY(5), // CLEARTYPE_QUALITY
        FF_DONTCARE.0 as u32,
        PCWSTR(wide_name.as_ptr()),
    )
}

unsafe extern "system" fn edit_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let parent = windows::Win32::UI::WindowsAndMessaging::GetParent(hwnd);
    let state_ptr = GetWindowLongPtrW(parent, GWLP_USERDATA) as *mut NativeCardState;

    if msg == WM_KEYDOWN {
        let vk = wparam.0 as i32;
        let ctrl_down = (GetKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000) != 0;

        if vk == VK_ESCAPE.0 as i32 {
            let _ = DestroyWindow(parent);
            return LRESULT(0);
        } else if vk == VK_RETURN.0 as i32 && ctrl_down {
            if !state_ptr.is_null() {
                trigger_copy_and_close(parent, &mut *state_ptr);
            }
            return LRESULT(0);
        } else if (vk == 'R' as i32 || vk == 'r' as i32) && ctrl_down {
            if !state_ptr.is_null() {
                let owner = (*state_ptr).owner;
                let _ = PostMessageW(Some(owner), WM_RETRY_ACTION, WPARAM(0), LPARAM(0));
                let _ = DestroyWindow(parent);
            }
            return LRESULT(0);
        }
    }

    if !state_ptr.is_null() && (*state_ptr).orig_edit_proc.is_some() {
        CallWindowProcW((*state_ptr).orig_edit_proc, hwnd, msg, wparam, lparam)
    } else {
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

unsafe fn trigger_copy_and_close(hwnd: HWND, state: &mut NativeCardState) {
    let len = GetWindowTextLengthW(state.edit_hwnd);
    if len > 0 {
        let mut buf = vec![0_u16; (len + 1) as usize];
        let copied = GetWindowTextW(state.edit_hwnd, &mut buf);
        if copied > 0 {
            let text = String::from_utf16_lossy(&buf[..copied as usize]);
            let _ = clipboard::write_text(&text);
        }
    }
    let _ = DestroyWindow(hwnd);
}

unsafe extern "system" fn native_card_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCCREATE => {
            let create_struct = lparam.0 as *const CREATESTRUCTW;
            if !create_struct.is_null() && !(*create_struct).lpCreateParams.is_null() {
                let params = Box::from_raw((*create_struct).lpCreateParams as *mut CreateParams);
                let dpi = GetDpiForWindow(hwnd).max(96);
                let is_dark = is_system_dark_mode();

                let state = Box::new(NativeCardState {
                    owner: params.owner,
                    data: params.data,
                    edit_hwnd: HWND::default(),
                    btn_retry_hwnd: HWND::default(),
                    btn_copy_hwnd: HWND::default(),
                    btn_close_hwnd: HWND::default(),
                    orig_edit_proc: None,
                    theme: CardThemeColors::new(is_dark),
                    font_ui_bold: create_gdi_font("Segoe UI Variable Text", 12, 600, dpi),
                    font_ui_normal: create_gdi_font("Segoe UI Variable Text", 11, 400, dpi),
                    font_ui_small: create_gdi_font("Segoe UI Variable Text", 10, 400, dpi),
                    font_mono: create_gdi_font("Cascadia Code", 12, 400, dpi),
                    activated_once: false,
                });
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        _ => {
            let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut NativeCardState;
            if pointer.is_null() {
                return DefWindowProcW(hwnd, message, wparam, lparam);
            }
            let state = &mut *pointer;

            match message {
                WM_CREATE => {
                    let dpi = GetDpiForWindow(hwnd).max(96);
                    let mut client_rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut client_rect);

                    let header_h = scale(38, dpi);
                    let footer_h = scale(44, dpi);
                    let pad = scale(12, dpi);

                    let edit_x = pad;
                    let edit_y = header_h + scale(8, dpi);
                    let edit_w = (client_rect.right - client_rect.left) - (pad * 2);
                    let edit_h =
                        (client_rect.bottom - client_rect.top) - edit_y - footer_h - scale(8, dpi);

                    let module = GetModuleHandleW(None).unwrap_or(HINSTANCE(std::ptr::null_mut()));
                    let wide_content = wide(&state.data.result_text);

                    let edit_hwnd = CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        w!("EDIT"),
                        PCWSTR(wide_content.as_ptr()),
                        WINDOW_STYLE(
                            WS_CHILD.0
                                | WS_VISIBLE.0
                                | ES_MULTILINE.0
                                | ES_AUTOVSCROLL.0
                                | ES_WANTRETURN.0
                                | WS_VSCROLL.0
                                | WS_TABSTOP.0,
                        ),
                        edit_x,
                        edit_y,
                        edit_w,
                        edit_h,
                        Some(hwnd),
                        Some(HMENU(IDC_EDIT as *mut c_void)),
                        Some(HINSTANCE(module.0)),
                        None,
                    )
                    .unwrap_or_default();

                    let orig_proc = SetWindowLongPtrW(
                        edit_hwnd,
                        GWLP_WNDPROC,
                        edit_subclass_proc as usize as isize,
                    );
                    state.orig_edit_proc = std::mem::transmute(orig_proc);
                    state.edit_hwnd = edit_hwnd;

                    let _ = SendMessageW(
                        edit_hwnd,
                        WM_SETFONT,
                        WPARAM(state.font_mono.0 as usize),
                        LPARAM(1),
                    );

                    // 底部按钮
                    let is_en = state.data.language == "en";
                    let retry_label = wide(if is_en { "Regenerate" } else { "重试" });
                    let copy_label = wide(if is_en { "Copy" } else { "复制修改" });

                    let btn_h = scale(28, dpi);
                    let btn_copy_w = scale(80, dpi);
                    let btn_retry_w = scale(68, dpi);
                    let btn_y = client_rect.bottom - footer_h + (footer_h - btn_h) / 2;

                    let btn_copy_x = client_rect.right - pad - btn_copy_w;
                    let btn_retry_x = btn_copy_x - scale(8, dpi) - btn_retry_w;

                    state.btn_retry_hwnd = CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        w!("BUTTON"),
                        PCWSTR(retry_label.as_ptr()),
                        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0),
                        btn_retry_x,
                        btn_y,
                        btn_retry_w,
                        btn_h,
                        Some(hwnd),
                        Some(HMENU(IDC_BTN_RETRY as *mut c_void)),
                        Some(HINSTANCE(module.0)),
                        None,
                    )
                    .unwrap_or_default();

                    state.btn_copy_hwnd = CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        w!("BUTTON"),
                        PCWSTR(copy_label.as_ptr()),
                        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0),
                        btn_copy_x,
                        btn_y,
                        btn_copy_w,
                        btn_h,
                        Some(hwnd),
                        Some(HMENU(IDC_BTN_COPY as *mut c_void)),
                        Some(HINSTANCE(module.0)),
                        None,
                    )
                    .unwrap_or_default();

                    // 关闭按钮 (右上角 ✕)
                    let close_size = scale(24, dpi);
                    let close_x = client_rect.right - pad - close_size + scale(4, dpi);
                    let close_y = (header_h - close_size) / 2;
                    let close_label = wide("✕");
                    state.btn_close_hwnd = CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        w!("BUTTON"),
                        PCWSTR(close_label.as_ptr()),
                        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
                        close_x,
                        close_y,
                        close_size,
                        close_size,
                        Some(hwnd),
                        Some(HMENU(IDC_BTN_CLOSE as *mut c_void)),
                        Some(HINSTANCE(module.0)),
                        None,
                    )
                    .unwrap_or_default();

                    let _ = SendMessageW(
                        state.btn_retry_hwnd,
                        WM_SETFONT,
                        WPARAM(state.font_ui_normal.0 as usize),
                        LPARAM(1),
                    );
                    let _ = SendMessageW(
                        state.btn_copy_hwnd,
                        WM_SETFONT,
                        WPARAM(state.font_ui_bold.0 as usize),
                        LPARAM(1),
                    );
                    let _ = SendMessageW(
                        state.btn_close_hwnd,
                        WM_SETFONT,
                        WPARAM(state.font_ui_small.0 as usize),
                        LPARAM(1),
                    );

                    // 自动全选或光标移至末尾，聚焦编辑框
                    let text_len = state.data.result_text.encode_utf16().count();
                    let _ = SendMessageW(
                        edit_hwnd,
                        EM_SETSEL,
                        WPARAM(text_len),
                        LPARAM(text_len as isize),
                    );
                    let _ = SetFocus(Some(edit_hwnd));

                    LRESULT(0)
                }
                WM_ACTIVATE => {
                    let activation_type = (wparam.0 & 0xFFFF) as u32;
                    if activation_type == WA_INACTIVE {
                        if state.activated_once {
                            let _ = DestroyWindow(hwnd);
                        }
                    } else {
                        state.activated_once = true;
                    }
                    LRESULT(0)
                }
                WM_COMMAND => {
                    let id = (wparam.0 & 0xFFFF) as usize;
                    match id {
                        IDC_BTN_COPY => {
                            trigger_copy_and_close(hwnd, state);
                            LRESULT(0)
                        }
                        IDC_BTN_RETRY => {
                            let owner = state.owner;
                            let _ = PostMessageW(Some(owner), WM_RETRY_ACTION, WPARAM(0), LPARAM(0));
                            let _ = DestroyWindow(hwnd);
                            LRESULT(0)
                        }
                        IDC_BTN_CLOSE => {
                            let _ = DestroyWindow(hwnd);
                            LRESULT(0)
                        }
                        _ => DefWindowProcW(hwnd, message, wparam, lparam),
                    }
                }
                WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
                    let hdc = HDC(wparam.0 as *mut c_void);
                    let control_hwnd = HWND(lparam.0 as *mut c_void);

                    if control_hwnd == state.edit_hwnd {
                        let _ = SetTextColor(hdc, state.theme.text_mono);
                        let _ = SetBkColor(hdc, state.theme.bg_edit);
                        return LRESULT(state.theme.brush_edit.0 as isize);
                    }
                    SetBkMode(hdc, TRANSPARENT);
                    SetTextColor(hdc, state.theme.text_primary);
                    LRESULT(state.theme.brush_bar.0 as isize)
                }
                WM_PAINT => {
                    let mut paint = PAINTSTRUCT::default();
                    let dc = BeginPaint(hwnd, &mut paint);
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    let dpi = GetDpiForWindow(hwnd).max(96);

                    // 1. 绘制整体外壳底色
                    let _ = FillRect(dc, &rect, state.theme.brush_shell);

                    let header_h = scale(38, dpi);
                    let footer_h = scale(44, dpi);
                    let pad = scale(12, dpi);

                    // 2. 绘制 Header 条
                    let header_rect = RECT {
                        left: 0,
                        top: 0,
                        right: rect.right,
                        bottom: header_h,
                    };
                    let _ = FillRect(dc, &header_rect, state.theme.brush_bar);

                    // Header 分割线
                    let old_pen = SelectObject(dc, state.theme.pen_border.into());
                    windows::Win32::Graphics::Gdi::MoveToEx(dc, 0, header_h, None);
                    windows::Win32::Graphics::Gdi::LineTo(dc, rect.right, header_h);

                    // 3. 绘制 Header 文本与徽章
                    let _ = SetBkMode(dc, TRANSPARENT);

                    // 状态微圆点
                    let dot_size = scale(6, dpi);
                    let dot_x = pad;
                    let dot_y = (header_h - dot_size) / 2;
                    let old_brush = SelectObject(dc, state.theme.brush_edit.into());
                    let _ = RoundRect(
                        dc,
                        dot_x,
                        dot_y,
                        dot_x + dot_size,
                        dot_y + dot_size,
                        dot_size,
                        dot_size,
                    );

                    // 动作名称
                    let font_bold = SelectObject(dc, state.font_ui_bold.into());
                    let _ = SetTextColor(dc, state.theme.text_primary);
                    let mut title_buf = wide(&state.data.action_name);
                    let mut title_rect = RECT {
                        left: dot_x + dot_size + scale(6, dpi),
                        top: 0,
                        right: rect.right / 2,
                        bottom: header_h,
                    };
                    let _ = DrawTextW(
                        dc,
                        &mut title_buf,
                        &mut title_rect,
                        windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT(
                            DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0,
                        ),
                    );

                    // 计算已复制徽章位置
                    let title_len = state.data.action_name.chars().count() as i32;
                    let badge_x = title_rect.left + scale(title_len * 13, dpi).min(scale(110, dpi));
                    let badge_w = scale(54, dpi);
                    let badge_h = scale(20, dpi);
                    let badge_y = (header_h - badge_h) / 2;

                    let _ = SelectObject(dc, state.theme.pen_badge_success.into());
                    let _ = SelectObject(dc, state.theme.brush_badge_success.into());
                    let _ = RoundRect(
                        dc,
                        badge_x,
                        badge_y,
                        badge_x + badge_w,
                        badge_y + badge_h,
                        scale(4, dpi),
                        scale(4, dpi),
                    );

                    // 徽章文字 "✓ 已复制"
                    let is_en = state.data.language == "en";
                    let mut badge_text = wide(if is_en { "✓ Copied" } else { "✓ 已复制" });
                    let mut badge_text_rect = RECT {
                        left: badge_x,
                        top: badge_y,
                        right: badge_x + badge_w,
                        bottom: badge_y + badge_h,
                    };
                    let font_small = SelectObject(dc, state.font_ui_small.into());
                    let _ = SetTextColor(dc, state.theme.badge_success_text);
                    let _ = DrawTextW(
                        dc,
                        &mut badge_text,
                        &mut badge_text_rect,
                        windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT(
                            windows::Win32::Graphics::Gdi::DT_CENTER.0
                                | DT_VCENTER.0
                                | DT_SINGLELINE.0,
                        ),
                    );

                    // 模型标签 (右侧)
                    let close_btn_w = scale(28, dpi);
                    let model_max_w = scale(130, dpi);
                    let model_right = rect.right - pad - close_btn_w;
                    let model_left = model_right - model_max_w;
                    let mut model_buf = wide(&state.data.model);
                    let mut model_rect = RECT {
                        left: model_left,
                        top: 0,
                        right: model_right,
                        bottom: header_h,
                    };
                    let _ = SetTextColor(dc, state.theme.text_tertiary);
                    let font_mono = SelectObject(dc, state.font_mono.into());
                    let _ = DrawTextW(
                        dc,
                        &mut model_buf,
                        &mut model_rect,
                        windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT(
                            windows::Win32::Graphics::Gdi::DT_RIGHT.0
                                | DT_VCENTER.0
                                | DT_SINGLELINE.0
                                | DT_END_ELLIPSIS.0,
                        ),
                    );

                    // 4. 绘制 Footer 条
                    let footer_rect = RECT {
                        left: 0,
                        top: rect.bottom - footer_h,
                        right: rect.right,
                        bottom: rect.bottom,
                    };
                    let _ = FillRect(dc, &footer_rect, state.theme.brush_bar);

                    // Footer 顶部分割线
                    let _ = SelectObject(dc, state.theme.pen_border.into());
                    windows::Win32::Graphics::Gdi::MoveToEx(dc, 0, rect.bottom - footer_h, None);
                    windows::Win32::Graphics::Gdi::LineTo(dc, rect.right, rect.bottom - footer_h);

                    // Footer 统计文字（字符 / 行数）
                    let char_count = state.data.result_text.chars().count();
                    let line_count = state.data.result_text.lines().count().max(1);
                    let stats_str = format!("{char_count} 字符 / {line_count} 行 · Esc 关闭 · Ctrl+Enter 复制");
                    let mut stats_buf = wide(&stats_str);
                    let mut stats_rect = RECT {
                        left: pad,
                        top: rect.bottom - footer_h,
                        right: rect.right - scale(180, dpi),
                        bottom: rect.bottom,
                    };
                    let _ = SetTextColor(dc, state.theme.text_tertiary);
                    let _ = SelectObject(dc, state.font_ui_small.into());
                    let _ = DrawTextW(
                        dc,
                        &mut stats_buf,
                        &mut stats_rect,
                        windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT(
                            DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0,
                        ),
                    );

                    // 恢复 GDI 对象
                    let _ = SelectObject(dc, font_bold);
                    let _ = SelectObject(dc, font_small);
                    let _ = SelectObject(dc, font_mono);
                    let _ = SelectObject(dc, old_pen);
                    let _ = SelectObject(dc, old_brush);

                    let _ = EndPaint(hwnd, &paint);
                    LRESULT(0)
                }
                WM_ERASEBKGND => LRESULT(1),
                WM_DESTROY => {
                    if let Ok(mut lock) = ACTIVE_RESULT_CARD_HWND.lock() {
                        if *lock == Some(hwnd.0 as isize) {
                            *lock = None;
                        }
                    }
                    let _ = PostMessageW(
                        Some(state.owner),
                        WM_RESULT_CARD_CLOSED,
                        WPARAM(hwnd.0 as usize),
                        LPARAM(0),
                    );
                    state.theme.release();
                    unsafe {
                        let _ = DeleteObject(state.font_ui_bold.into());
                        let _ = DeleteObject(state.font_ui_normal.into());
                        let _ = DeleteObject(state.font_ui_small.into());
                        let _ = DeleteObject(state.font_mono.into());
                    }
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    drop(Box::from_raw(pointer));
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, message, wparam, lparam),
            }
        }
    }
}
