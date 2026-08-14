use prompt_optimizer::config::{ApiProfile, Config};
use serde::Deserialize;
use std::ffi::c_void;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2, ICoreWebView2Controller,
};
use webview2_com::{
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    WebMessageReceivedEventHandler,
};
use windows::core::{w, Error as WindowsError, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowLongPtrW, IsWindow,
    LoadCursorW, MessageBoxW, PostMessageW, RegisterClassW, SendMessageW, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, CREATESTRUCTW, CW_USEDEFAULT, GWLP_USERDATA,
    HICON, ICON_BIG, ICON_SMALL, IDC_ARROW, MB_ICONERROR, MB_OK, MINMAXINFO, SWP_NOACTIVATE,
    SWP_NOZORDER, SW_SHOW, WM_APP, WM_CLOSE, WM_CREATE, WM_DESTROY, WM_DPICHANGED,
    WM_GETMINMAXINFO, WM_NCCREATE, WM_SETICON, WM_SIZE, WNDCLASSW, WS_CAPTION, WS_CLIPCHILDREN,
    WS_EX_APPWINDOW, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, WS_THICKFRAME,
};

pub const WM_APPLY_CONFIG: u32 = WM_APP + 20;
pub const WM_SETTINGS_CLOSED: u32 = WM_APP + 21;
pub const WM_TEST_API: u32 = WM_APP + 22;
pub const WM_TEST_TRANSLATION_API: u32 = WM_APP + 23;
pub const WM_TEST_ACTION_API: u32 = WM_APP + 24;

const CLASS_NAME: PCWSTR = w!("PromptOptimizer.SettingsWindow");
const TITLE: PCWSTR = w!("PromptOptimizer 设置");
const SETTINGS_HTML: &str = include_str!("settings.html");

pub struct ApplyRequest {
    pub config: Config,
    pub error: Option<String>,
}

pub struct ApiTestRequest {
    pub config: Config,
    pub error: Option<String>,
}

pub struct ActionApiTestRequest {
    pub config: Config,
    pub action: prompt_optimizer::config::CustomAction,
    pub error: Option<String>,
}

struct CreateParams<'a> {
    owner: HWND,
    config: &'a Config,
}

struct SettingsState {
    owner: HWND,
    current: Config,
    draft_profiles: Vec<ApiProfile>,
    draft_active_profile: String,
    controller: Option<ICoreWebView2Controller>,
    webview: Option<ICoreWebView2>,
    webview_user_data_dir: Option<std::path::PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct WebFormData {
    profile_name: String,
    api_key: String,
    base_url: String,
    models: Vec<String>,
    model: String,
    translation_model: String,
    temperature: String,
    max_tokens: String,
    system_prompt: String,
    translation_prompt: String,
    hotkey: String,
    translation_hotkey: String,
    native_language: String,
    target_language: String,
    #[serde(default)]
    actions: Vec<prompt_optimizer::config::CustomAction>,
    play_sound: bool,
    auto_start: bool,
}

fn stage_active_profile(
    profiles: &mut [ApiProfile],
    active_profile: &mut String,
    form: &WebFormData,
) -> Result<(), String> {
    let active_index = profiles
        .iter()
        .position(|profile| {
            profile
                .name
                .trim()
                .eq_ignore_ascii_case(active_profile.trim())
        })
        .ok_or_else(|| format!("当前 API 配置不存在：{}", active_profile.trim()))?;
    let temperature = form
        .temperature
        .trim()
        .parse::<f64>()
        .map_err(|_| "温度必须是 0.0–2.0 之间的数字".to_string())?;
    let max_tokens = form
        .max_tokens
        .trim()
        .parse::<u32>()
        .map_err(|_| "最大 Token 数必须是大于 0 的整数".to_string())?;
    let mut models = Vec::new();
    for model in &form.models {
        let model = model.trim().to_string();
        if !model.is_empty() && !models.contains(&model) {
            models.push(model);
        }
    }
    let selected_model = form.model.trim().to_string();
    if models.is_empty() && !selected_model.is_empty() {
        models.push(selected_model.clone());
    }
    let selected_translation_model = if form.translation_model.trim().is_empty() {
        selected_model.clone()
    } else {
        form.translation_model.trim().to_string()
    };
    let profile = ApiProfile {
        name: form.profile_name.trim().to_string(),
        api_key: form.api_key.clone(),
        base_url: form.base_url.trim().to_string(),
        models,
        model: selected_model,
        translation_model: selected_translation_model,
        temperature,
        max_tokens,
    };
    profile.validate().map_err(|error| error.to_string())?;
    if profiles.iter().enumerate().any(|(index, item)| {
        index != active_index && item.name.trim().eq_ignore_ascii_case(profile.name.trim())
    }) {
        return Err(format!("API 配置名称重复：{}", profile.name));
    }
    *active_profile = profile.name.clone();
    profiles[active_index] = profile;
    Ok(())
}

fn parse_form_data(value: &serde_json::Value) -> Result<WebFormData, String> {
    serde_json::from_value(value.clone()).map_err(|error| format!("设置表单数据无效：{error}"))
}

fn config_from_form(
    current: &Config,
    profiles: &[ApiProfile],
    active_profile: &str,
    form: &WebFormData,
) -> Result<Config, String> {
    let mut config = current.clone();
    config.api_profiles = profiles.to_vec();
    config.active_profile = active_profile.to_string();
    stage_active_profile(&mut config.api_profiles, &mut config.active_profile, form)?;
    config.system_prompt = form.system_prompt.clone();
    let trans_prompt = form.translation_prompt.trim();
    config.translation_prompt = if trans_prompt.is_empty() {
        current.translation_prompt.clone()
    } else {
        form.translation_prompt.clone()
    };
    config.hotkey = form.hotkey.trim().to_string();
    let trans_hotkey = form.translation_hotkey.trim();
    config.translation_hotkey = if trans_hotkey.is_empty() {
        current.translation_hotkey.clone()
    } else {
        trans_hotkey.to_string()
    };
    let native_lang = form.native_language.trim();
    config.native_language = if native_lang.is_empty() {
        current.native_language.clone()
    } else {
        native_lang.to_string()
    };
    let target_lang = form.target_language.trim();
    config.target_language = if target_lang.is_empty() {
        current.target_language.clone()
    } else {
        target_lang.to_string()
    };

    if !form.actions.is_empty() {
        config.actions = form.actions.clone();
        if let Some(opt) = config.actions.iter().find(|a| {
            a.id.eq_ignore_ascii_case(prompt_optimizer::config::DEFAULT_OPTIMIZE_ACTION_ID)
        }) {
            config.hotkey = opt.hotkey.clone();
            config.system_prompt = opt.system_prompt.clone();
        }
        if let Some(trans) = config.actions.iter().find(|a| {
            a.id.eq_ignore_ascii_case(prompt_optimizer::config::DEFAULT_TRANSLATE_ACTION_ID)
        }) {
            config.translation_hotkey = trans.hotkey.clone();
            config.translation_prompt = trans.system_prompt.clone();
        }
    }

    config.play_sound = form.play_sound;
    config.auto_start = form.auto_start;
    config.validate().map_err(|error| error.to_string())?;
    Ok(config)
}

fn stage_form_in_state(
    state: &mut SettingsState,
    value: &serde_json::Value,
) -> Result<WebFormData, String> {
    let form = parse_form_data(value)?;
    stage_active_profile(
        &mut state.draft_profiles,
        &mut state.draft_active_profile,
        &form,
    )?;
    Ok(form)
}

fn next_profile_name(profiles: &[ApiProfile]) -> String {
    (1_u32..)
        .map(|index| format!("新配置 {index}"))
        .find(|candidate| {
            !profiles
                .iter()
                .any(|profile| profile.name.trim().eq_ignore_ascii_case(candidate))
        })
        .expect("配置名称序号不会耗尽")
}

pub fn register(instance: HINSTANCE) -> Result<(), WindowsError> {
    let class = WNDCLASSW {
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }?,
        hInstance: instance,
        lpszClassName: CLASS_NAME,
        lpfnWndProc: Some(window_proc),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(WindowsError::from_thread());
    }
    Ok(())
}

pub unsafe fn show(
    owner: HWND,
    instance: HINSTANCE,
    icon: HICON,
    config: &Config,
) -> Result<HWND, WindowsError> {
    let dpi = GetDpiForWindow(owner).max(96);
    let style = WS_OVERLAPPED
        | WS_CAPTION
        | WS_SYSMENU
        | WS_THICKFRAME
        | WS_MINIMIZEBOX
        | WS_MAXIMIZEBOX
        | WS_CLIPCHILDREN;
    let ex_style = WS_EX_APPWINDOW;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: scale(860, dpi),
        bottom: scale(730, dpi),
    };
    let _ = AdjustWindowRectExForDpi(&mut rect, style, false, ex_style, dpi);
    let params = CreateParams { owner, config };
    let hwnd = CreateWindowExW(
        ex_style,
        CLASS_NAME,
        TITLE,
        style,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        rect.right - rect.left,
        rect.bottom - rect.top,
        Some(owner),
        None,
        Some(instance),
        Some((&params as *const CreateParams<'_>).cast::<c_void>()),
    )?;
    let _ = SendMessageW(
        hwnd,
        WM_SETICON,
        Some(WPARAM(ICON_SMALL as usize)),
        Some(LPARAM(icon.0 as isize)),
    );
    let _ = SendMessageW(
        hwnd,
        WM_SETICON,
        Some(WPARAM(ICON_BIG as usize)),
        Some(LPARAM(icon.0 as isize)),
    );
    let preference = DWMWCP_ROUND;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        std::ptr::from_ref(&preference).cast::<c_void>(),
        size_of_val(&preference) as u32,
    );
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
    Ok(hwnd)
}

pub unsafe fn focus(hwnd: HWND) {
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
}

pub unsafe fn refresh(hwnd: HWND, config: &Config) {
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SettingsState;
    if pointer.is_null() {
        return;
    }
    let state = &mut *pointer;
    state.current = config.clone();
    state.draft_profiles = config.api_profiles.clone();
    state.draft_active_profile = config.active_profile.clone();
    send_state_to_web(state, Some("配置已重新刷新"), false, false);
}

pub unsafe fn complete_api_test(hwnd: HWND, result: Result<(), String>) {
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SettingsState;
    if pointer.is_null() {
        return;
    }
    let state = &mut *pointer;
    match result {
        Ok(()) => send_status_to_web(state, "API 连接正常", false, true, false),
        Err(error) => send_status_to_web(state, &error, true, false, false),
    }
}

pub unsafe fn complete_translation_api_test(hwnd: HWND, result: Result<(), String>) {
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SettingsState;
    if pointer.is_null() {
        return;
    }
    let state = &mut *pointer;
    match result {
        Ok(()) => send_status_to_web(state, "翻译 API 连接正常", false, true, false),
        Err(error) => send_status_to_web(state, &error, true, false, false),
    }
}

pub unsafe fn complete_action_api_test(hwnd: HWND, action_name: &str, result: Result<(), String>) {
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SettingsState;
    if pointer.is_null() {
        return;
    }
    let state = &mut *pointer;
    match result {
        Ok(()) => {
            let msg = format!("「{}」API 连接正常（HTTP 200）", action_name);
            send_status_to_web(state, &msg, false, true, false);
        }
        Err(error) => {
            let msg = format!("「{}」API 测试失败：{}", action_name, error);
            send_status_to_web(state, &msg, true, false, false);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = lparam.0 as *const CREATESTRUCTW;
        if !create.is_null() {
            let params = (*create).lpCreateParams as *const CreateParams<'_>;
            if !params.is_null() {
                let state = Box::new(SettingsState {
                    owner: (*params).owner,
                    current: (*params).config.clone(),
                    draft_profiles: (*params).config.api_profiles.clone(),
                    draft_active_profile: (*params).config.active_profile.clone(),
                    controller: None,
                    webview: None,
                    webview_user_data_dir: None,
                });
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
            }
        }
    }

    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SettingsState;
    if pointer.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let state = &mut *pointer;

    match message {
        WM_CREATE => {
            init_webview(hwnd);
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
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let info = lparam.0 as *mut MINMAXINFO;
            if !info.is_null() {
                let dpi = GetDpiForWindow(hwnd).max(96);
                (*info).ptMinTrackSize.x = scale(720, dpi);
                (*info).ptMinTrackSize.y = scale(760, dpi);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = PostMessageW(
                Some(state.owner),
                WM_SETTINGS_CLOSED,
                WPARAM(hwnd.0 as usize),
                LPARAM(0),
            );
            if let Some(controller) = state.controller.take() {
                let _ = unsafe { controller.Close() };
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

fn init_webview(hwnd: HWND) {
    let (user_data_dir, user_data_path) = match create_webview_user_data_dir() {
        Ok(value) => value,
        Err(error) => {
            show_webview_error(hwnd, "设置页临时目录创建失败", &error.to_string());
            return;
        }
    };
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SettingsState };
    if !pointer.is_null() {
        unsafe { &mut *pointer }.webview_user_data_dir = Some(user_data_dir);
    }
    let env_handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
        move |result, environment| {
            if let Err(error) = result {
                show_webview_error(hwnd, "WebView2 运行环境创建失败", &error.to_string());
                return Ok(());
            }
            if let Some(env) = environment {
                let ctrl_handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
                    move |res, controller| {
                        if let Err(error) = res {
                            show_webview_error(hwnd, "WebView2 控制器创建失败", &error.to_string());
                            return Ok(());
                        }
                        if let Some(controller) = controller {
                            if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
                                return Ok(());
                            }
                            let pointer = unsafe {
                                GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SettingsState
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
                                                "设置页消息通道创建失败",
                                                &error.to_string(),
                                            );
                                            return Ok(());
                                        }
                                        let html_wide = wide(SETTINGS_HTML);
                                        if let Err(error) = unsafe {
                                            webview.NavigateToString(PCWSTR(html_wide.as_ptr()))
                                        } {
                                            show_webview_error(
                                                hwnd,
                                                "设置页面加载失败",
                                                &error.to_string(),
                                            );
                                        }
                                    }
                                    Err(error) => {
                                        show_webview_error(
                                            hwnd,
                                            "WebView2 页面实例创建失败",
                                            &error.to_string(),
                                        );
                                    }
                                }
                            }
                        } else {
                            show_webview_error(hwnd, "WebView2 控制器不可用", "未返回控制器实例");
                        }
                        Ok(())
                    },
                ));
                if let Err(error) = unsafe { env.CreateCoreWebView2Controller(hwnd, &ctrl_handler) }
                {
                    show_webview_error(hwnd, "WebView2 控制器启动失败", &error.to_string());
                }
            } else {
                show_webview_error(hwnd, "WebView2 运行环境不可用", "未返回运行环境实例");
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
        show_webview_error(hwnd, "WebView2 启动失败", &error.to_string());
    }
}

fn create_webview_user_data_dir() -> std::io::Result<(std::path::PathBuf, Vec<u16>)> {
    static NEXT_DIR_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT_DIR_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("PromptOptimizer_WV2_{}_{}", std::process::id(), id));
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

fn show_webview_error(hwnd: HWND, context: &str, detail: &str) {
    if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        return;
    }
    let message = wide(&format!(
        "{context}。请确认已安装 Microsoft Edge WebView2 Runtime。\n\n{detail}"
    ));
    unsafe {
        MessageBoxW(
            Some(hwnd),
            PCWSTR(message.as_ptr()),
            w!("PromptOptimizer 设置"),
            MB_OK | MB_ICONERROR,
        );
        let _ = DestroyWindow(hwnd);
    }
}

fn handle_web_message(hwnd: HWND, json_str: &str) {
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SettingsState };
    if pointer.is_null() {
        return;
    }
    let state = unsafe { &mut *pointer };
    let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) else {
        return;
    };
    let action = val["action"].as_str().unwrap_or_default();
    match action {
        "ready" | "reset" => {
            state.draft_profiles = state.current.api_profiles.clone();
            state.draft_active_profile = state.current.active_profile.clone();
            send_state_to_web(state, None, false, false);
        }
        "switch_profile" => {
            if let Err(error) = stage_form_in_state(state, &val["data"]) {
                send_status_to_web(state, &error, true, false, false);
                return;
            }
            if let Some(name) = val["name"].as_str() {
                state.draft_active_profile = name.to_string();
                send_profile_state_to_web(state, "配置已修改（未保存）");
            }
        }
        "profile_new" => {
            if let Err(error) = stage_form_in_state(state, &val["data"]) {
                send_status_to_web(state, &error, true, false, false);
                return;
            }
            let new_name = next_profile_name(&state.draft_profiles);
            let new_p = ApiProfile {
                name: new_name.clone(),
                ..ApiProfile::default()
            };
            state.draft_profiles.push(new_p);
            state.draft_active_profile = new_name;
            send_profile_state_to_web(state, "已创建新配置（未保存）");
        }
        "profile_delete" => {
            if let Err(error) = stage_form_in_state(state, &val["data"]) {
                send_status_to_web(state, &error, true, false, false);
                return;
            }
            if state.draft_profiles.len() > 1 {
                let current = state.draft_active_profile.clone();
                state.draft_profiles.retain(|p| p.name != current);
                if let Some(first) = state.draft_profiles.first() {
                    state.draft_active_profile = first.name.clone();
                }
                send_profile_state_to_web(state, "已删除配置（未保存）");
            } else {
                send_state_to_web(state, Some("至少需保留一个配置"), true, false);
            }
        }
        "save" => {
            save_from_web(hwnd, state, &val["data"]);
        }
        "test_api" => {
            test_api_from_web(hwnd, state, &val["data"]);
        }
        "test_translation_api" => {
            test_translation_api_from_web(hwnd, state, &val["data"]);
        }
        "test_action_api" => {
            let action_id = val["action_id"].as_str().unwrap_or("optimize");
            test_action_api_from_web(hwnd, state, &val["data"], action_id);
        }
        _ => {}
    }
}

fn test_action_api_from_web(
    hwnd: HWND,
    state: &mut SettingsState,
    data: &serde_json::Value,
    action_id: &str,
) {
    let form = match parse_form_data(data) {
        Ok(form) => form,
        Err(error) => {
            send_status_to_web(state, &error, true, false, false);
            return;
        }
    };
    let config = match config_from_form(
        &state.current,
        &state.draft_profiles,
        &state.draft_active_profile,
        &form,
    ) {
        Ok(config) => config,
        Err(error) => {
            send_status_to_web(state, &error, true, false, false);
            return;
        }
    };
    let action = config
        .find_action(action_id)
        .cloned()
        .unwrap_or_else(|| config.actions.first().cloned().unwrap_or_default());

    send_status_to_web(
        state,
        &format!("正在测试「{}」API 连接...", action.name),
        false,
        false,
        true,
    );
    let mut request = ActionApiTestRequest {
        config,
        action,
        error: None,
    };
    let result = unsafe {
        SendMessageW(
            state.owner,
            WM_TEST_ACTION_API,
            Some(WPARAM(hwnd.0 as usize)),
            Some(LPARAM((&mut request as *mut ActionApiTestRequest) as isize)),
        )
    };
    if result.0 != 1 {
        let message = request.error.as_deref().unwrap_or("API 测试未能启动");
        send_status_to_web(state, message, true, false, false);
    }
}

fn save_from_web(hwnd: HWND, state: &mut SettingsState, data: &serde_json::Value) {
    let form = match parse_form_data(data) {
        Ok(form) => form,
        Err(error) => {
            send_status_to_web(state, &error, true, false, false);
            return;
        }
    };
    let config = match config_from_form(
        &state.current,
        &state.draft_profiles,
        &state.draft_active_profile,
        &form,
    ) {
        Ok(config) => config,
        Err(error) => {
            send_status_to_web(state, &error, true, false, false);
            return;
        }
    };

    let mut request = ApplyRequest {
        config,
        error: None,
    };
    let result = unsafe {
        SendMessageW(
            state.owner,
            WM_APPLY_CONFIG,
            Some(WPARAM(hwnd.0 as usize)),
            Some(LPARAM((&mut request as *mut ApplyRequest) as isize)),
        )
    };
    if result.0 == 1 {
        state.current = request.config;
        state.draft_profiles = state.current.api_profiles.clone();
        state.draft_active_profile = state.current.active_profile.clone();
        send_state_to_web(state, Some("已保存并应用"), false, true);
    } else {
        let msg = request
            .error
            .as_deref()
            .unwrap_or("配置未能应用，请重写验证后提交");
        send_status_to_web(state, msg, true, false, false);
    }
}

fn test_api_from_web(hwnd: HWND, state: &mut SettingsState, data: &serde_json::Value) {
    let form = match parse_form_data(data) {
        Ok(form) => form,
        Err(error) => {
            send_status_to_web(state, &error, true, false, false);
            return;
        }
    };
    let config = match config_from_form(
        &state.current,
        &state.draft_profiles,
        &state.draft_active_profile,
        &form,
    ) {
        Ok(config) => config,
        Err(error) => {
            send_status_to_web(state, &error, true, false, false);
            return;
        }
    };

    send_status_to_web(state, "正在测试 API 连接...", false, false, true);
    let mut request = ApiTestRequest {
        config,
        error: None,
    };
    let result = unsafe {
        SendMessageW(
            state.owner,
            WM_TEST_API,
            Some(WPARAM(hwnd.0 as usize)),
            Some(LPARAM((&mut request as *mut ApiTestRequest) as isize)),
        )
    };
    if result.0 != 1 {
        let message = request.error.as_deref().unwrap_or("API 测试未能启动");
        send_status_to_web(state, message, true, false, false);
    }
}

fn test_translation_api_from_web(hwnd: HWND, state: &mut SettingsState, data: &serde_json::Value) {
    let form = match parse_form_data(data) {
        Ok(form) => form,
        Err(error) => {
            send_status_to_web(state, &error, true, false, false);
            return;
        }
    };
    let config = match config_from_form(
        &state.current,
        &state.draft_profiles,
        &state.draft_active_profile,
        &form,
    ) {
        Ok(config) => config,
        Err(error) => {
            send_status_to_web(state, &error, true, false, false);
            return;
        }
    };

    send_status_to_web(state, "正在测试翻译 API 连接...", false, false, true);
    let mut request = ApiTestRequest {
        config,
        error: None,
    };
    let result = unsafe {
        SendMessageW(
            state.owner,
            WM_TEST_TRANSLATION_API,
            Some(WPARAM(hwnd.0 as usize)),
            Some(LPARAM((&mut request as *mut ApiTestRequest) as isize)),
        )
    };
    if result.0 != 1 {
        let message = request.error.as_deref().unwrap_or("翻译 API 测试未能启动");
        send_status_to_web(state, message, true, false, false);
    }
}

fn send_state_to_web(
    state: &SettingsState,
    status: Option<&str>,
    is_error: bool,
    is_success: bool,
) {
    let Some(webview) = &state.webview else {
        return;
    };
    let current_p = state
        .draft_profiles
        .iter()
        .find(|p| p.name == state.draft_active_profile)
        .cloned()
        .unwrap_or_default();

    let json = serde_json::json!({
        "profiles": state.draft_profiles,
        "active_profile": state.draft_active_profile,
        "current_profile": current_p,
        "system_prompt": state.current.system_prompt,
        "translation_prompt": state.current.translation_prompt,
        "hotkey": state.current.hotkey,
        "translation_hotkey": state.current.translation_hotkey,
        "native_language": state.current.native_language,
        "target_language": state.current.target_language,
        "actions": state.current.actions,
        "play_sound": state.current.play_sound,
        "auto_start": state.current.auto_start,
        "status": status.unwrap_or("所有修改统一点击“保存并应用”"),
        "is_error": is_error,
        "is_success": is_success,
    });
    let script = format!("window.updateConfig({});", json);
    let wide_script = wide(&script);
    let _ = unsafe { webview.ExecuteScript(PCWSTR(wide_script.as_ptr()), None) };
}

fn send_profile_state_to_web(state: &SettingsState, status: &str) {
    let Some(webview) = &state.webview else {
        return;
    };
    let current_profile = state
        .draft_profiles
        .iter()
        .find(|profile| profile.name == state.draft_active_profile)
        .cloned()
        .unwrap_or_default();
    let payload = serde_json::json!({
        "profiles": state.draft_profiles,
        "active_profile": state.draft_active_profile,
        "current_profile": current_profile,
        "status": status,
    });
    let script = format!("window.updateProfileState({payload});");
    let wide_script = wide(&script);
    let _ = unsafe { webview.ExecuteScript(PCWSTR(wide_script.as_ptr()), None) };
}

fn send_status_to_web(
    state: &SettingsState,
    status: &str,
    is_error: bool,
    is_success: bool,
    is_testing: bool,
) {
    let Some(webview) = &state.webview else {
        return;
    };
    let args = serde_json::json!([status, is_error, is_success, is_testing]);
    let script = format!("window.updateStatus(...{});", args);
    let wide_script = wide(&script);
    let _ = unsafe { webview.ExecuteScript(PCWSTR(wide_script.as_ptr()), None) };
}

fn scale(value: i32, dpi: u32) -> i32 {
    ((value as i64 * dpi as i64 + 48) / 96) as i32
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renaming_the_active_profile_keeps_it_active() {
        let mut profiles = vec![ApiProfile::default()];
        let mut active_profile = profiles[0].name.clone();
        let form = WebFormData {
            profile_name: "工作配置".into(),
            api_key: "secret".into(),
            base_url: "https://example.com/v1".into(),
            model: "example-model".into(),
            temperature: "0.3".into(),
            max_tokens: "512".into(),
            ..WebFormData::default()
        };

        stage_active_profile(&mut profiles, &mut active_profile, &form).unwrap();

        assert_eq!(active_profile, "工作配置");
        assert_eq!(profiles[0].name, "工作配置");
    }

    #[test]
    fn invalid_numeric_fields_are_rejected_without_changing_the_profile() {
        let original = ApiProfile::default();
        let mut profiles = vec![original.clone()];
        let mut active_profile = original.name.clone();
        let form = WebFormData {
            profile_name: original.name.clone(),
            api_key: original.api_key.clone(),
            base_url: original.base_url.clone(),
            model: original.model.clone(),
            temperature: "not-a-number".into(),
            max_tokens: original.max_tokens.to_string(),
            ..WebFormData::default()
        };

        let error = stage_active_profile(&mut profiles, &mut active_profile, &form).unwrap_err();

        assert!(error.contains("温度必须"));
        assert_eq!(profiles, vec![original]);
    }

    #[test]
    fn zero_temperature_remains_a_valid_explicit_value() {
        let current = Config::default();
        let profile = current.api_profiles[0].clone();
        let form = WebFormData {
            profile_name: profile.name.clone(),
            api_key: profile.api_key.clone(),
            base_url: profile.base_url.clone(),
            models: profile.models.clone(),
            model: profile.model.clone(),
            temperature: "0".into(),
            max_tokens: profile.max_tokens.to_string(),
            system_prompt: current.system_prompt.clone(),
            hotkey: current.hotkey.clone(),
            play_sound: current.play_sound,
            auto_start: current.auto_start,
            ..WebFormData::default()
        };

        let config = config_from_form(
            &current,
            &current.api_profiles,
            &current.active_profile,
            &form,
        )
        .unwrap();

        assert_eq!(config.active_api().unwrap().temperature, 0.0);
    }

    #[test]
    fn staged_profile_edits_survive_switching_to_another_profile() {
        let first = ApiProfile::default();
        let second = ApiProfile {
            name: "备用配置".into(),
            ..ApiProfile::default()
        };
        let mut profiles = vec![first.clone(), second];
        let mut active_profile = first.name.clone();
        let form = WebFormData {
            profile_name: first.name,
            api_key: first.api_key,
            base_url: first.base_url,
            model: "edited-before-switch".into(),
            temperature: first.temperature.to_string(),
            max_tokens: first.max_tokens.to_string(),
            ..WebFormData::default()
        };

        stage_active_profile(&mut profiles, &mut active_profile, &form).unwrap();
        active_profile = "备用配置".into();

        assert_eq!(profiles[0].model, "edited-before-switch");
        assert_eq!(active_profile, "备用配置");
    }

    #[test]
    fn new_profile_names_do_not_collide_with_existing_gaps() {
        let profiles = vec![
            ApiProfile {
                name: "新配置 1".into(),
                ..ApiProfile::default()
            },
            ApiProfile {
                name: "新配置 3".into(),
                ..ApiProfile::default()
            },
        ];

        assert_eq!(next_profile_name(&profiles), "新配置 2");
    }

    #[test]
    fn form_round_trip_preserves_translation_model_and_other_profiles() {
        let mut current = Config::default();
        let secondary = ApiProfile {
            name: "备用配置".into(),
            models: vec!["backup-model".into(), "backup-translation".into()],
            model: "backup-model".into(),
            translation_model: "backup-translation".into(),
            ..ApiProfile::default()
        };
        current.api_profiles.push(secondary.clone());
        let form = WebFormData {
            profile_name: current.active_profile.clone(),
            api_key: current.api_profiles[0].api_key.clone(),
            base_url: current.api_profiles[0].base_url.clone(),
            models: vec!["main-model".into(), "main-translation".into()],
            model: "main-model".into(),
            translation_model: "main-translation".into(),
            temperature: "0.3".into(),
            max_tokens: "512".into(),
            system_prompt: current.system_prompt.clone(),
            translation_prompt: current.translation_prompt.clone(),
            hotkey: current.hotkey.clone(),
            translation_hotkey: current.translation_hotkey.clone(),
            native_language: current.native_language.clone(),
            target_language: current.target_language.clone(),
            play_sound: current.play_sound,
            auto_start: current.auto_start,
            ..WebFormData::default()
        };

        let saved = config_from_form(
            &current,
            &current.api_profiles,
            &current.active_profile,
            &form,
        )
        .unwrap();

        assert_eq!(saved.active_api().unwrap().model, "main-model");
        assert_eq!(
            saved.active_api().unwrap().translation_model,
            "main-translation"
        );
        assert_eq!(saved.api_profiles[1], secondary);
    }

    #[test]
    fn staging_a_profile_preserves_multiple_models_and_the_selection() {
        let mut profiles = vec![ApiProfile::default()];
        let mut active_profile = profiles[0].name.clone();
        let form = WebFormData {
            profile_name: active_profile.clone(),
            base_url: "https://example.com/v1".into(),
            models: vec!["model-a".into(), "model-b".into()],
            model: "model-b".into(),
            temperature: "0.3".into(),
            max_tokens: "512".into(),
            ..WebFormData::default()
        };

        stage_active_profile(&mut profiles, &mut active_profile, &form).unwrap();

        assert_eq!(profiles[0].models, vec!["model-a", "model-b"]);
        assert_eq!(profiles[0].model, "model-b");
    }
}
