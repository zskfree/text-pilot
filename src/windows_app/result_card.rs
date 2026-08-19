use super::clipboard;
use serde::{Deserialize, Serialize};
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Mutex;
use text_pilot::config::UiLanguage;
use text_pilot::i18n::{self, Message};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2, ICoreWebView2Controller,
};
use webview2_com::{
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    WebMessageReceivedEventHandler,
};
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetCursorPos, GetWindowLongPtrW,
    IsWindow, LoadCursorW, MessageBoxW, PostMessageW, RegisterClassW, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, CREATESTRUCTW, CW_USEDEFAULT, GWLP_USERDATA,
    IDC_ARROW, MB_ICONERROR, MB_OK, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_SHOW, WA_INACTIVE,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_ACTIVATE, WM_APP, WM_CLOSE, WM_CREATE, WM_DESTROY,
    WM_DPICHANGED, WM_NCCREATE, WM_SIZE, WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

pub const WM_RESULT_CARD_CLOSED: u32 = WM_APP + 30;
pub const WM_RETRY_ACTION: u32 = WM_APP + 31;

const CLASS_NAME: PCWSTR = w!("TextPilot.ResultCardWindow");
const RESULT_CARD_HTML: &str = include_str!("result_card.html");

const DEFAULT_CARD_WIDTH: i32 = 480;
const DEFAULT_CARD_HEIGHT: i32 = 320;

static ACTIVE_RESULT_CARD_HWND: Mutex<Option<isize>> = Mutex::new(None);

#[derive(Clone, Debug, Serialize, Deserialize)]
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

struct ResultCardState {
    owner: HWND,
    data: ResultCardData,
    controller: Option<ICoreWebView2Controller>,
    webview: Option<ICoreWebView2>,
    webview_user_data_dir: Option<PathBuf>,
    is_ready: bool,
    activated_once: bool,
}

#[derive(Deserialize)]
struct WebMessagePayload {
    action: String,
    #[serde(default)]
    text: String,
}

pub fn show_result_card(owner: HWND, data: ResultCardData) {
    // 若已有打开的卡片窗口，先将其安全关闭
    close_existing_card();

    let module = match unsafe { GetModuleHandleW(None) } {
        Ok(handle) => handle,
        Err(_) => return,
    };

    let class = WNDCLASSW {
        lpfnWndProc: Some(result_card_window_proc),
        hInstance: HINSTANCE(module.0),
        lpszClassName: CLASS_NAME,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() },
        ..Default::default()
    };
    let _ = unsafe { RegisterClassW(&class) };

    // 获取选区锚点或鼠标位置
    let dpi = 96; // 初始基准 DPI，窗口创建后可根据 HWND 重取
    let (initial_x, initial_y, width, height) = calculate_card_bounds(dpi);

    let params = Box::new(CreateParams { owner, data });
    let params_ptr = Box::into_raw(params);

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0),
            CLASS_NAME,
            w!("TextPilot Result Card"),
            WINDOW_STYLE(WS_POPUP.0),
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

    // 应用 Windows 11 DWM 圆角
    let preference = DWMWCP_ROUND;
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            std::ptr::from_ref(&preference).cast(),
            std::mem::size_of_val(&preference) as u32,
        )
    };

    // 保存当前激活的卡片 HWND
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

    // 默认定位在选区正下方或鼠标下方
    let mut x = anchor.left;
    let mut y = anchor.bottom + gap;

    // 水平防出界
    if x + width > work.right {
        x = work.right - width - gap;
    }
    if x < work.left + gap {
        x = work.left + gap;
    }

    // 垂直防出界：如果下方放不下，则移到上方
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

unsafe extern "system" fn result_card_window_proc(
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
                let state = Box::new(ResultCardState {
                    owner: params.owner,
                    data: params.data,
                    controller: None,
                    webview: None,
                    webview_user_data_dir: None,
                    is_ready: false,
                    activated_once: false,
                });
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        _ => {
            let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ResultCardState;
            if pointer.is_null() {
                return DefWindowProcW(hwnd, message, wparam, lparam);
            }
            let state = &mut *pointer;

            match message {
                WM_CREATE => {
                    init_webview(hwnd);
                    LRESULT(0)
                }
                WM_ACTIVATE => {
                    let activation_type = (wparam.0 & 0xFFFF) as u32;
                    if activation_type == WA_INACTIVE {
                        // 失焦且此前已激活过 -> 自动消失
                        if state.activated_once {
                            let _ = DestroyWindow(hwnd);
                        }
                    } else {
                        state.activated_once = true;
                    }
                    LRESULT(0)
                }
                WM_SIZE => {
                    if let Some(controller) = &state.controller {
                        let mut rect = RECT::default();
                        let _ = GetClientRect(hwnd, &mut rect);
                        let _ = controller.SetBounds(rect);
                    }
                    LRESULT(0)
                }
                WM_DPICHANGED => {
                    let suggested = lparam.0 as *const RECT;
                    if !suggested.is_null() {
                        let rect = *suggested;
                        let _ = SetWindowPos(
                            hwnd,
                            None,
                            rect.left,
                            rect.top,
                            rect.right - rect.left,
                            rect.bottom - rect.top,
                            SWP_NOACTIVATE,
                        );
                    }
                    LRESULT(0)
                }
                WM_CLOSE => {
                    let _ = DestroyWindow(hwnd);
                    LRESULT(0)
                }
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
                    if let Some(controller) = state.controller.take() {
                        let _ = controller.Close();
                    }
                    state.webview = None;
                    let user_data_dir = state.webview_user_data_dir.take();
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    drop(Box::from_raw(pointer));
                    if let Some(dir) = user_data_dir {
                        std::thread::spawn(move || clean_webview_temp_dir(&dir));
                    }
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, message, wparam, lparam),
            }
        }
    }
}

fn init_webview(hwnd: HWND) {
    let (user_data_dir, user_data_path) = match create_webview_user_data_dir() {
        Ok(value) => value,
        Err(error) => {
            show_webview_error(
                hwnd,
                Message::WebViewTempDirectoryFailed,
                &error.to_string(),
            );
            return;
        }
    };
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ResultCardState };
    if !pointer.is_null() {
        unsafe { &mut *pointer }.webview_user_data_dir = Some(user_data_dir);
    }

    let env_handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
        move |result, environment| {
            if let Err(error) = result {
                show_webview_error(hwnd, Message::WebViewEnvironmentFailed, &error.to_string());
                return Ok(());
            }
            if let Some(env) = environment {
                let ctrl_handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
                    move |res, controller| {
                        if let Err(error) = res {
                            show_webview_error(
                                hwnd,
                                Message::WebViewControllerFailed,
                                &error.to_string(),
                            );
                            return Ok(());
                        }
                        if let Some(controller) = controller {
                            if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
                                return Ok(());
                            }
                            let pointer = unsafe {
                                GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ResultCardState
                            };
                            if !pointer.is_null() {
                                let state = unsafe { &mut *pointer };
                                state.controller = Some(controller.clone());

                                let mut rect = RECT::default();
                                unsafe {
                                    let _ = GetClientRect(hwnd, &mut rect);
                                    let _ = controller.SetBounds(rect);
                                    let _ = controller.SetIsVisible(true);
                                }

                                match unsafe { controller.CoreWebView2() } {
                                    Ok(webview) => {
                                        state.webview = Some(webview.clone());
                                        let msg_handler = WebMessageReceivedEventHandler::create(
                                            Box::new(move |_sender, args| {
                                                if let Some(args) = args {
                                                    let mut json_ptr = PWSTR::null();
                                                    if unsafe {
                                                        args.WebMessageAsJson(&mut json_ptr)
                                                    }
                                                    .is_ok()
                                                        && !json_ptr.is_null()
                                                    {
                                                        let json_str = unsafe {
                                                            json_ptr.to_string().unwrap_or_default()
                                                        };
                                                        unsafe {
                                                            CoTaskMemFree(Some(json_ptr.0.cast()));
                                                        }
                                                        handle_web_message(hwnd, &json_str);
                                                    }
                                                }
                                                Ok(())
                                            }),
                                        );
                                        let mut token = 0_i64;
                                        if let Err(error) = unsafe {
                                            webview.add_WebMessageReceived(
                                                &msg_handler,
                                                std::ptr::from_mut(&mut token),
                                            )
                                        } {
                                            show_webview_error(
                                                hwnd,
                                                Message::WebViewMessageChannelFailed,
                                                &error.to_string(),
                                            );
                                            return Ok(());
                                        }

                                        let html_wide = wide(RESULT_CARD_HTML);
                                        if let Err(error) = unsafe {
                                            webview.NavigateToString(PCWSTR(html_wide.as_ptr()))
                                        } {
                                            show_webview_error(
                                                hwnd,
                                                Message::WebViewPageLoadFailed,
                                                &error.to_string(),
                                            );
                                        }
                                    }
                                    Err(error) => {
                                        show_webview_error(
                                            hwnd,
                                            Message::WebViewInstanceFailed,
                                            &error.to_string(),
                                        );
                                    }
                                }
                            }
                        } else {
                            show_webview_error(hwnd, Message::WebViewControllerUnavailable, "");
                        }
                        Ok(())
                    },
                ));
                if let Err(error) = unsafe { env.CreateCoreWebView2Controller(hwnd, &ctrl_handler) }
                {
                    show_webview_error(
                        hwnd,
                        Message::WebViewControllerStartFailed,
                        &error.to_string(),
                    );
                }
            } else {
                show_webview_error(hwnd, Message::WebViewEnvironmentUnavailable, "");
            }
            Ok(())
        },
    ));

    if let Err(error) = unsafe {
        CreateCoreWebView2EnvironmentWithOptions(
            None,
            PCWSTR(user_data_path.as_ptr()),
            None,
            &env_handler,
        )
    } {
        show_webview_error(hwnd, Message::WebViewEnvironmentStartFailed, &error.to_string());
    }
}

fn handle_web_message(hwnd: HWND, json_str: &str) {
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ResultCardState };
    if pointer.is_null() {
        return;
    }
    let state = unsafe { &mut *pointer };

    let payload: WebMessagePayload = match serde_json::from_str(json_str) {
        Ok(parsed) => parsed,
        Err(_) => return,
    };

    match payload.action.as_str() {
        "ready" => {
            state.is_ready = true;
            if let Some(webview) = &state.webview {
                if let Ok(data_json) = serde_json::to_string(&state.data) {
                    let script = format!("window.setCardData({});", data_json);
                    let wide_script = wide(&script);
                    let _ = unsafe {
                        webview.ExecuteScript(PCWSTR(wide_script.as_ptr()), None)
                    };
                }
            }
        }
        "copy_text" => {
            // 用户在卡片内修改后点击复制或按快捷键复制
            if !payload.text.is_empty() {
                let _ = clipboard::write_text(&payload.text);
            }
        }
        "retry_action" => {
            // 请求重新生成
            let owner = state.owner;
            unsafe {
                let _ = PostMessageW(Some(owner), WM_RETRY_ACTION, WPARAM(0), LPARAM(0));
                let _ = DestroyWindow(hwnd);
            }
        }
        "close_card" => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
        _ => {}
    }
}

fn create_webview_user_data_dir() -> std::io::Result<(PathBuf, Vec<u16>)> {
    static NEXT_DIR_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(100);
    let id = NEXT_DIR_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("TextPilot_Card_WV2_{}_{}", std::process::id(), id));
    std::fs::create_dir_all(&dir)?;
    let wide_path = wide(&dir.to_string_lossy());
    Ok((dir, wide_path))
}

fn clean_webview_temp_dir(dir: &std::path::Path) {
    std::thread::sleep(std::time::Duration::from_millis(200));
    if dir.exists() {
        for _ in 0..6 {
            if std::fs::remove_dir_all(dir).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    }
}

fn show_webview_error(hwnd: HWND, message: Message, detail: &str) {
    if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        return;
    }
    let language = UiLanguage::ChineseSimplified;
    let title = wide(i18n::text(language, Message::AppName));
    let message = wide(&format!(
        "{}\n\n{}\n\n{detail}",
        i18n::text(language, message),
        i18n::text(language, Message::WebViewRuntimeRequired),
    ));
    unsafe {
        let _ = MessageBoxW(
            Some(hwnd),
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
        let _ = DestroyWindow(hwnd);
    }
}
