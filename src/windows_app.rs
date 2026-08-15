mod clipboard;
mod selection;
mod settings;
mod startup;

use std::ffi::c_void;
use std::fmt::{Display, Formatter};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::{
    mpsc::{self, Receiver, Sender},
    Mutex,
};
use std::thread;
use text_pilot::api::ApiClient;
use text_pilot::config::{self, Config, ConfigError, CustomAction, UiLanguage};
use text_pilot::hotkey::{parse_hotkey, HotkeyKind};
use text_pilot::i18n::{self, Message};
use windows::core::{w, Error as WindowsError, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, COLORREF, ERROR_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT,
    POINT, RECT, WPARAM,
};
#[cfg(test)]
use windows::Win32::Graphics::Gdi::UpdateWindow;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreatePen, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint,
    GetMonitorInfoW, InvalidateRect, MonitorFromRect, RoundRect, SelectObject, SetBkMode,
    SetTextColor, SetWindowRgn, DT_END_ELLIPSIS, DT_SINGLELINE, DT_VCENTER, HFONT, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, VK_CONTROL,
};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CallNextHookEx, CreateIconFromResourceEx, CreatePopupMenu, CreateWindowExW,
    DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, GetClientRect,
    GetCursorPos, GetForegroundWindow, GetMessageW, GetWindowLongPtrW, IsWindow, KillTimer,
    LoadCursorW, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW,
    RegisterWindowMessageW, SetForegroundWindow, SetLayeredWindowAttributes, SetTimer,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, SetWindowsHookExW, ShowWindow, TrackPopupMenu,
    TranslateMessage, UnhookWindowsHookEx, CREATESTRUCTW, CW_USEDEFAULT, GWLP_USERDATA, HHOOK,
    HICON, HWND_TOPMOST, IDC_ARROW, IMAGE_FLAGS, KBDLLHOOKSTRUCT, LLKHF_INJECTED, LR_DEFAULTCOLOR,
    LWA_ALPHA, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MF_CHECKED, MF_POPUP, MF_SEPARATOR,
    MF_STRING, MSG, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, TPM_BOTTOMALIGN, TPM_LEFTALIGN,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, WH_KEYBOARD_LL, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
    WM_CONTEXTMENU, WM_DESTROY, WM_ERASEBKGND, WM_HOTKEY, WM_KEYDOWN, WM_KEYUP, WM_NCCREATE,
    WM_NULL, WM_PAINT, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

const APP_NAME: PCWSTR = w!("TextPilot");
const WINDOW_CLASS: PCWSTR = w!("TextPilot.HiddenWindow");
const STATUS_WINDOW_CLASS: PCWSTR = w!("TextPilot.StatusPopup");
const BASE_HOTKEY_ID: i32 = 0x5000;

use text_pilot::api::ActionTier;

const TRAY_ID: u32 = 1;
const WM_TRAY: u32 = WM_APP + 1;
const WM_WORKER_DONE: u32 = WM_APP + 2;
const WM_GESTURE_HOTKEY: u32 = WM_APP + 3;
const MENU_SETTINGS: u32 = 1001;
const MENU_RELOAD: u32 = 1002;
const MENU_EXIT: u32 = 1003;
const MENU_LANGUAGE_ENGLISH: u32 = 1004;
const MENU_LANGUAGE_CHINESE: u32 = 1005;
const MENU_LAST_ERROR: u32 = 1006;
const STATUS_HEIGHT: i32 = 34;
const STATUS_MIN_WIDTH: i32 = 112;
const STATUS_MAX_WIDTH: i32 = 300;
const ICON_FILE: &[u8] = include_bytes!("../assets/text-pilot.ico");
const GESTURE_TIMER_ID: usize = 2;
const GESTURE_INTERVAL_MS: u32 = 520;
const TIER_DISPATCH_WINDOW_MS: u32 = 280;

#[derive(Clone, Copy)]
enum PopupSide {
    Right,
    Left,
}

#[derive(Clone, Copy)]
struct PopupAnchor {
    side: PopupSide,
    edge_x: i32,
    y: i32,
    max_width: i32,
    work_left: i32,
    work_right: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum StatusKind {
    #[default]
    Neutral,
    Progress,
    Success,
    Error,
}

struct StatusPopupState {
    hwnd: isize,
    visible: bool,
    anchor: Option<PopupAnchor>,
    kind: StatusKind,
}

static STATUS_POPUP: Mutex<StatusPopupState> = Mutex::new(StatusPopupState {
    hwnd: 0,
    visible: false,
    anchor: None,
    kind: StatusKind::Neutral,
});
static LAST_ERROR: Mutex<Option<(String, String)>> = Mutex::new(None);

#[derive(Clone, Debug, Default)]
struct GestureSlot {
    double_action_index: Option<u32>,
    triple_action_index: Option<u32>,
    virtual_key: u32,
    double_has_deep_tier: bool,
    taps: u8,
    key_down: bool,
    last_tap_time: u32,
    sequence: u64,
}

impl GestureSlot {
    fn has_triple_target(&self) -> bool {
        self.triple_action_index.is_some()
            || (self.double_action_index.is_some() && self.double_has_deep_tier)
    }

    fn action_for_tier(&self, tier: ActionTier) -> Option<(u32, ActionTier)> {
        match tier {
            ActionTier::Standard => self
                .double_action_index
                .map(|index| (index, ActionTier::Standard)),
            ActionTier::Deep => self
                .triple_action_index
                .map(|index| (index, ActionTier::Deep))
                .or_else(|| {
                    self.double_has_deep_tier
                        .then_some(self.double_action_index)
                        .flatten()
                        .map(|index| (index, ActionTier::Deep))
                }),
        }
    }
}

struct GestureHookState {
    hook: isize,
    hwnd: isize,
    slots: Vec<GestureSlot>,
    next_sequence: u64,
}

fn advance_gesture_tap(
    current_taps: u8,
    last_tap_time: u32,
    now: u32,
    has_triple_tier: bool,
) -> (u8, u8, Option<ActionTier>) {
    let expired = current_taps > 0 && now.wrapping_sub(last_tap_time) > GESTURE_INTERVAL_MS;
    let replay = if expired { current_taps } else { 0 };
    let taps = if expired {
        1
    } else {
        current_taps.saturating_add(1)
    };
    if taps >= 3 {
        (0, replay, Some(ActionTier::Deep))
    } else if taps == 2 && !has_triple_tier {
        (0, replay, Some(ActionTier::Standard))
    } else {
        (taps, replay, None)
    }
}

type GestureDispatch = Option<(u32, ActionTier)>;
type GestureReplays = Vec<(u8, u32)>;

fn process_pending_gestures(
    slots: &mut [GestureSlot],
    except_virtual_key: Option<u32>,
) -> (GestureDispatch, GestureReplays) {
    let mut triggered = None;
    let mut pending = Vec::new();
    for slot in slots {
        if slot.taps == 2 && except_virtual_key != Some(slot.virtual_key) {
            if triggered.is_none() {
                triggered = slot.action_for_tier(ActionTier::Standard);
            }
            if triggered.is_none() {
                pending.push((slot.sequence, slot.taps, slot.virtual_key));
            }
            slot.taps = 0;
            slot.sequence = 0;
        } else if slot.taps == 1 && except_virtual_key != Some(slot.virtual_key) {
            pending.push((slot.sequence, slot.taps, slot.virtual_key));
            slot.taps = 0;
            slot.sequence = 0;
        }
    }
    pending.sort_by_key(|(sequence, _, _)| *sequence);
    let replays = pending
        .into_iter()
        .map(|(_, taps, virtual_key)| (taps, virtual_key))
        .collect();
    (triggered, replays)
}

static GESTURE_HOOK: Mutex<GestureHookState> = Mutex::new(GestureHookState {
    hook: 0,
    hwnd: 0,
    slots: Vec::new(),
    next_sequence: 1,
});

impl PopupAnchor {
    fn position(self, width: i32) -> (i32, i32) {
        let width = width.min(self.max_width);
        let x = match self.side {
            PopupSide::Right => self.edge_x,
            PopupSide::Left => self.edge_x - width,
        }
        .clamp(
            self.work_left,
            (self.work_right - width).max(self.work_left),
        );
        (x, self.y)
    }
}

#[derive(Debug)]
pub struct AppError(String);

impl Display for AppError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for AppError {}

impl From<WindowsError> for AppError {
    fn from(value: WindowsError) -> Self {
        Self(value.to_string())
    }
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> Result<Self, WindowsError> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

enum WorkerCommand {
    ExecuteAction {
        task_id: u64,
        action: CustomAction,
        tier: ActionTier,
        config: Config,
        text: String,
    },
    TestApi {
        settings_hwnd: isize,
        config: Config,
    },
    TestTranslationApi {
        settings_hwnd: isize,
        config: Config,
    },
    TestActionApi {
        settings_hwnd: isize,
        action: CustomAction,
        config: Config,
    },
    Shutdown,
}

enum WorkerResult {
    ExecuteAction {
        task_id: u64,
        action_name: String,
        result: Result<String, String>,
    },
    TestApi {
        settings_hwnd: isize,
        result: Result<(), String>,
    },
    TestTranslationApi {
        settings_hwnd: isize,
        result: Result<(), String>,
    },
    TestActionApi {
        settings_hwnd: isize,
        action_name: String,
        result: Result<(), String>,
    },
}

struct AppState {
    config: Config,
    config_path: PathBuf,
    exe_path: PathBuf,
    hotkeys_registered: bool,
    busy: bool,
    next_task_id: u64,
    active_task_id: Option<u64>,
    worker_tx: Sender<WorkerCommand>,
    worker_rx: Receiver<WorkerResult>,
    icon: HICON,
    taskbar_created: u32,
    settings_hwnd: HWND,
}

fn idle_tooltip_text(config: &Config) -> String {
    let active_actions: Vec<String> = config
        .actions
        .iter()
        .filter(|a| a.enabled)
        .take(3)
        .map(|a| format!("{}: {}", a.name, a.hotkey))
        .collect();
    if active_actions.is_empty() {
        i18n::text(config.ui_language, Message::NoHotkeys).into()
    } else {
        format!(
            "{} | {}",
            i18n::text(config.ui_language, Message::Running),
            active_actions.join(" | ")
        )
    }
}

pub fn run() -> Result<(), AppError> {
    let _com_apartment = ComApartment::initialize()?;
    let exe_path = std::env::current_exe().map_err(|error| AppError(error.to_string()))?;
    let config_path = exe_path.with_file_name("config.json");
    let (config, first_run, startup_warning) = load_startup_config(&config_path)?;
    // Keep the pre-v0.5 mutex name so upgraded and legacy builds cannot run together.
    let mutex = unsafe { CreateMutexW(None, true, w!("Local\\PromptOptimizer.SingleInstance")) }?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        let message = wide(i18n::text(config.ui_language, Message::AlreadyRunning));
        unsafe {
            MessageBoxW(
                None,
                PCWSTR(message.as_ptr()),
                APP_NAME,
                MB_OK | MB_ICONINFORMATION,
            );
            let _ = CloseHandle(mutex);
        }
        return Ok(());
    }

    let instance = HINSTANCE(unsafe { GetModuleHandleW(None) }?.0);
    register_window_class(instance)?;
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            WINDOW_CLASS,
            APP_NAME,
            WINDOW_STYLE::default(),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            None,
            None,
            Some(instance),
            None,
        )
    }?;

    let icon = create_embedded_icon(32)?;
    let (command_tx, command_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    start_worker(command_rx, result_tx, hwnd.0 as isize);

    let mut state = Box::new(AppState {
        config,
        config_path,
        exe_path,
        hotkeys_registered: false,
        busy: false,
        next_task_id: 1,
        active_task_id: None,
        worker_tx: command_tx,
        worker_rx: result_rx,
        icon,
        taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
        settings_hwnd: HWND::default(),
    });
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&mut *state as *mut AppState) as isize);
    }

    add_tray_icon(hwnd, &state)?;
    state.hotkeys_registered = activate_hotkeys(hwnd, &state.config).is_ok();
    if !state.hotkeys_registered {
        notify(
            hwnd,
            state.icon,
            i18n::text(state.config.ui_language, Message::InternalError),
            i18n::text(state.config.ui_language, Message::HotkeyRegistrationFailed),
            true,
        );
    }
    if let Err(error) = startup::set_auto_start(state.config.auto_start, &state.exe_path) {
        notify(
            hwnd,
            state.icon,
            i18n::text(state.config.ui_language, Message::AutoStartFailed),
            &error.to_string(),
            true,
        );
    }
    if let Some(warning) = startup_warning {
        notify(
            hwnd,
            state.icon,
            i18n::text(state.config.ui_language, Message::ConfigReset),
            &warning,
            true,
        );
    } else if first_run {
        notify(
            hwnd,
            state.icon,
            i18n::text(state.config.ui_language, Message::FirstRun),
            i18n::text(state.config.ui_language, Message::FirstRunMessage),
            false,
        );
    }
    if first_run {
        unsafe {
            open_config(hwnd, &mut state);
        }
    }

    let mut message = MSG::default();
    let message_loop_error = loop {
        let status = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if status.0 == -1 {
            break Some(AppError(WindowsError::from_thread().to_string()));
        }
        if status.0 == 0 {
            break None;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    };

    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
    }
    let _ = state.worker_tx.send(WorkerCommand::Shutdown);
    if state.hotkeys_registered {
        deactivate_hotkeys(hwnd, &state.config);
    }
    delete_tray_icon(hwnd);
    unsafe {
        if !state.settings_hwnd.is_invalid() && IsWindow(Some(state.settings_hwnd)).as_bool() {
            let _ = DestroyWindow(state.settings_hwnd);
        }
        let _ = DestroyIcon(state.icon);
        let _ = CloseHandle(mutex);
    }
    match message_loop_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn load_startup_config(path: &std::path::Path) -> Result<(Config, bool, Option<String>), AppError> {
    match config::load_or_create(path) {
        Ok((config, created)) => Ok((config, created, None)),
        Err(error @ ConfigError::InvalidJson { .. }) => {
            let warning = error.to_string();
            let config = config::load_existing(path).map_err(|next| AppError(next.to_string()))?;
            Ok((config, true, Some(warning)))
        }
        Err(error) => Err(AppError(error.to_string())),
    }
}

fn register_window_class(instance: HINSTANCE) -> Result<(), AppError> {
    let class = WNDCLASSW {
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }?,
        hInstance: instance,
        lpszClassName: WINDOW_CLASS,
        lpfnWndProc: Some(window_proc),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(AppError(WindowsError::from_thread().to_string()));
    }
    let status_class = WNDCLASSW {
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }?,
        hInstance: instance,
        lpszClassName: STATUS_WINDOW_CLASS,
        lpfnWndProc: Some(status_window_proc),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&status_class) } == 0 {
        return Err(AppError(WindowsError::from_thread().to_string()));
    }
    settings::register(instance).map_err(AppError::from)?;
    Ok(())
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match catch_unwind(AssertUnwindSafe(|| {
        window_proc_inner(hwnd, message, wparam, lparam)
    })) {
        Ok(result) => result,
        Err(_) => {
            let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;
            if !pointer.is_null() {
                (*pointer).busy = false;
                (*pointer).active_task_id = None;
            }
            LRESULT(0)
        }
    }
}

unsafe fn window_proc_inner(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        let create = lparam.0 as *const CREATESTRUCTW;
        if !create.is_null() && !(*create).lpCreateParams.is_null() {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*create).lpCreateParams as isize);
        }
    }

    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;
    if !pointer.is_null() {
        let state = &mut *pointer;
        if message == state.taskbar_created {
            let _ = add_tray_icon(hwnd, state);
            return LRESULT(0);
        }
        match message {
            WM_HOTKEY => {
                let id = wparam.0 as i32;
                if id >= BASE_HOTKEY_ID {
                    let action_index = (id - BASE_HOTKEY_ID) as usize;
                    on_action_triggered(hwnd, state, action_index, ActionTier::Standard);
                }
                return LRESULT(0);
            }
            WM_GESTURE_HOTKEY => {
                let action_index = wparam.0;
                let tier = if lparam.0 == 1 {
                    ActionTier::Deep
                } else {
                    ActionTier::Standard
                };
                on_action_triggered(hwnd, state, action_index, tier);
                return LRESULT(0);
            }
            WM_TIMER if wparam.0 == GESTURE_TIMER_ID => {
                handle_gesture_timer(hwnd);
                return LRESULT(0);
            }
            WM_TRAY => {
                let event = lparam.0 as u32;
                if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
                    show_tray_menu(hwnd, state);
                }
                return LRESULT(0);
            }
            WM_WORKER_DONE => {
                on_worker_done(hwnd, state);
                return LRESULT(0);
            }
            settings::WM_APPLY_CONFIG => {
                let request = lparam.0 as *mut settings::ApplyRequest;
                if request.is_null() {
                    return LRESULT(0);
                }
                match apply_config(hwnd, state, (*request).config.clone(), true) {
                    Ok(()) => {
                        (*request).error = None;
                        return LRESULT(1);
                    }
                    Err(error) => {
                        (*request).error = Some(error);
                        return LRESULT(0);
                    }
                }
            }
            settings::WM_SET_LANGUAGE => {
                let request = lparam.0 as *mut settings::LanguageRequest;
                if request.is_null() {
                    return LRESULT(0);
                }
                let previous = state.config.ui_language;
                state.config.ui_language = (*request).language;
                match config::save(&state.config_path, &state.config) {
                    Ok(()) => {
                        (*request).error = None;
                        update_tooltip(hwnd, state.icon, &idle_tooltip_text(&state.config));
                        return LRESULT(1);
                    }
                    Err(error) => {
                        state.config.ui_language = previous;
                        (*request).error = Some(error.localized_message((*request).language));
                        return LRESULT(0);
                    }
                }
            }
            settings::WM_TEST_API => {
                let request = lparam.0 as *mut settings::ApiTestRequest;
                if request.is_null() {
                    return LRESULT(0);
                }
                if state.busy {
                    (*request).error =
                        Some(i18n::text(state.config.ui_language, Message::Busy).into());
                    return LRESULT(0);
                }
                let settings_hwnd = wparam.0 as isize;
                state.busy = true;
                update_tooltip(
                    hwnd,
                    state.icon,
                    i18n::text(state.config.ui_language, Message::ApiTesting),
                );
                if state
                    .worker_tx
                    .send(WorkerCommand::TestApi {
                        settings_hwnd,
                        config: (*request).config.clone(),
                    })
                    .is_err()
                {
                    state.busy = false;
                    update_tooltip(hwnd, state.icon, &idle_tooltip_text(&state.config));
                    (*request).error = Some(
                        i18n::text(state.config.ui_language, Message::WorkerUnavailable).into(),
                    );
                    return LRESULT(0);
                }
                (*request).error = None;
                return LRESULT(1);
            }
            settings::WM_TEST_TRANSLATION_API => {
                let request = lparam.0 as *mut settings::ApiTestRequest;
                if request.is_null() {
                    return LRESULT(0);
                }
                if state.busy {
                    (*request).error =
                        Some(i18n::text(state.config.ui_language, Message::Busy).into());
                    return LRESULT(0);
                }
                let settings_hwnd = wparam.0 as isize;
                state.busy = true;
                update_tooltip(
                    hwnd,
                    state.icon,
                    i18n::text(state.config.ui_language, Message::TranslationApiTesting),
                );
                if state
                    .worker_tx
                    .send(WorkerCommand::TestTranslationApi {
                        settings_hwnd,
                        config: (*request).config.clone(),
                    })
                    .is_err()
                {
                    state.busy = false;
                    update_tooltip(hwnd, state.icon, &idle_tooltip_text(&state.config));
                    (*request).error = Some(
                        i18n::text(state.config.ui_language, Message::WorkerUnavailable).into(),
                    );
                    return LRESULT(0);
                }
                (*request).error = None;
                return LRESULT(1);
            }
            settings::WM_TEST_ACTION_API => {
                let request = lparam.0 as *mut settings::ActionApiTestRequest;
                if request.is_null() {
                    return LRESULT(0);
                }
                if state.busy {
                    (*request).error =
                        Some(i18n::text(state.config.ui_language, Message::Busy).into());
                    return LRESULT(0);
                }
                let settings_hwnd = wparam.0 as isize;
                let action_name = (*request).action.name.clone();
                state.busy = true;
                update_tooltip(
                    hwnd,
                    state.icon,
                    &i18n::format(
                        state.config.ui_language,
                        Message::ActionApiTesting,
                        &action_name,
                    ),
                );
                if state
                    .worker_tx
                    .send(WorkerCommand::TestActionApi {
                        settings_hwnd,
                        action: (*request).action.clone(),
                        config: (*request).config.clone(),
                    })
                    .is_err()
                {
                    state.busy = false;
                    update_tooltip(hwnd, state.icon, &idle_tooltip_text(&state.config));
                    (*request).error = Some(
                        i18n::text(state.config.ui_language, Message::WorkerUnavailable).into(),
                    );
                    return LRESULT(0);
                }
                (*request).error = None;
                return LRESULT(1);
            }
            settings::WM_SETTINGS_CLOSED => {
                if state.settings_hwnd.0 as usize == wparam.0 {
                    state.settings_hwnd = HWND::default();
                }
                return LRESULT(0);
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                return LRESULT(0);
            }
            _ => {}
        }
    } else if message == WM_DESTROY {
        PostQuitMessage(0);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}

unsafe fn on_action_triggered(
    hwnd: HWND,
    state: &mut AppState,
    action_index: usize,
    tier: ActionTier,
) {
    let language = state.config.ui_language;
    if state.busy {
        notify(
            hwnd,
            state.icon,
            "TextPilot",
            i18n::text(language, Message::Busy),
            false,
        );
        return;
    }
    let Some(action) = state.config.actions.get(action_index).cloned() else {
        return;
    };
    if !action.enabled {
        return;
    }
    if state
        .config
        .active_api()
        .is_none_or(|api| api.api_key.trim().is_empty())
    {
        notify(
            hwnd,
            state.icon,
            i18n::text(language, Message::MissingApiKeyTitle),
            i18n::text(language, Message::MissingApiKey),
            true,
        );
        return;
    }
    match selection::read_selected_text() {
        Ok(Some(text)) => {
            let task_id = state.next_task_id;
            state.next_task_id = state.next_task_id.wrapping_add(1).max(1);
            state.busy = true;
            state.active_task_id = Some(task_id);

            let action_name = action.name.clone();
            let message = match tier {
                ActionTier::Standard => i18n::format(language, Message::Processing, &action_name),
                ActionTier::Deep => i18n::format(language, Message::ProcessingDeep, &action_name),
            };

            let command = WorkerCommand::ExecuteAction {
                task_id,
                action,
                tier,
                config: state.config.clone(),
                text,
            };

            if state.worker_tx.send(command).is_err() {
                state.busy = false;
                state.active_task_id = None;
                update_tooltip(hwnd, state.icon, &idle_tooltip_text(&state.config));
                notify(
                    hwnd,
                    state.icon,
                    i18n::text(language, Message::InternalError),
                    i18n::text(language, Message::WorkerUnavailable),
                    true,
                );
                return;
            }

            update_tooltip(hwnd, state.icon, &message);
            show_status_popup(&message, StatusKind::Progress, true);
        }
        Ok(None) => notify(
            hwnd,
            state.icon,
            "TextPilot",
            i18n::text(language, Message::NoSelection),
            false,
        ),
        Err(error) => notify(
            hwnd,
            state.icon,
            i18n::text(language, Message::ReadSelectionFailed),
            &error.localized_message(language),
            true,
        ),
    }
}

unsafe fn on_worker_done(hwnd: HWND, state: &mut AppState) {
    let Ok(result) = state.worker_rx.try_recv() else {
        return;
    };
    match result {
        WorkerResult::ExecuteAction {
            task_id,
            action_name,
            result,
        } => {
            if state.active_task_id != Some(task_id) {
                return;
            }
            state.busy = false;
            state.active_task_id = None;
            update_tooltip(hwnd, state.icon, &idle_tooltip_text(&state.config));
            match result {
                Ok(text) => match clipboard::write_text(&text) {
                    Ok(()) => {
                        if state.config.play_sound {
                            let _ = MessageBeep(MB_OK);
                        }
                        show_status_popup(
                            i18n::text(state.config.ui_language, Message::Copied),
                            StatusKind::Success,
                            false,
                        );
                    }
                    Err(error) => notify(
                        hwnd,
                        state.icon,
                        i18n::text(state.config.ui_language, Message::ClipboardWriteFailed),
                        &error.to_string(),
                        true,
                    ),
                },
                Err(error) => notify(
                    hwnd,
                    state.icon,
                    &format!(
                        "{}: {}",
                        action_name,
                        i18n::text(state.config.ui_language, Message::InternalError)
                    ),
                    &error,
                    true,
                ),
            }
        }
        WorkerResult::TestApi {
            settings_hwnd,
            result,
        } => {
            state.busy = false;
            update_tooltip(hwnd, state.icon, &idle_tooltip_text(&state.config));
            let settings_hwnd = HWND(settings_hwnd as *mut c_void);
            if state.settings_hwnd == settings_hwnd && IsWindow(Some(settings_hwnd)).as_bool() {
                settings::complete_api_test(settings_hwnd, result);
            }
        }
        WorkerResult::TestTranslationApi {
            settings_hwnd,
            result,
        } => {
            state.busy = false;
            update_tooltip(hwnd, state.icon, &idle_tooltip_text(&state.config));
            let settings_hwnd = HWND(settings_hwnd as *mut c_void);
            if state.settings_hwnd == settings_hwnd && IsWindow(Some(settings_hwnd)).as_bool() {
                settings::complete_translation_api_test(settings_hwnd, result);
            }
        }
        WorkerResult::TestActionApi {
            settings_hwnd,
            action_name,
            result,
        } => {
            state.busy = false;
            update_tooltip(hwnd, state.icon, &idle_tooltip_text(&state.config));
            let settings_hwnd = HWND(settings_hwnd as *mut c_void);
            if state.settings_hwnd == settings_hwnd && IsWindow(Some(settings_hwnd)).as_bool() {
                settings::complete_action_api_test(settings_hwnd, &action_name, result);
            }
        }
    }
}

unsafe fn show_tray_menu(hwnd: HWND, state: &mut AppState) {
    let Ok(menu) = CreatePopupMenu() else {
        return;
    };
    let Ok(language_menu) = CreatePopupMenu() else {
        let _ = DestroyMenu(menu);
        return;
    };
    let language = state.config.ui_language;
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        MENU_SETTINGS as usize,
        PCWSTR(wide(i18n::text(language, Message::Settings)).as_ptr()),
    );
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        MENU_RELOAD as usize,
        PCWSTR(wide(i18n::text(language, Message::ReloadConfig)).as_ptr()),
    );
    let has_last_error = LAST_ERROR
        .lock()
        .map(|error| error.is_some())
        .unwrap_or(false);
    if has_last_error {
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            MENU_LAST_ERROR as usize,
            PCWSTR(wide(i18n::text(language, Message::ViewLastError)).as_ptr()),
        );
    }
    let english_flags = if language == UiLanguage::English {
        MF_STRING | MF_CHECKED
    } else {
        MF_STRING
    };
    let chinese_flags = if language == UiLanguage::ChineseSimplified {
        MF_STRING | MF_CHECKED
    } else {
        MF_STRING
    };
    let _ = AppendMenuW(
        language_menu,
        english_flags,
        MENU_LANGUAGE_ENGLISH as usize,
        w!("English"),
    );
    let _ = AppendMenuW(
        language_menu,
        chinese_flags,
        MENU_LANGUAGE_CHINESE as usize,
        w!("简体中文"),
    );
    let _ = AppendMenuW(
        menu,
        MF_POPUP,
        language_menu.0 as usize,
        PCWSTR(wide(i18n::text(language, Message::Language)).as_ptr()),
    );
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        MENU_EXIT as usize,
        PCWSTR(wide(i18n::text(language, Message::Exit)).as_ptr()),
    );
    let mut point = POINT::default();
    let _ = GetCursorPos(&mut point);
    let _ = SetForegroundWindow(hwnd);
    let command = TrackPopupMenu(
        menu,
        TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD,
        point.x,
        point.y,
        None,
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);
    let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
    match command.0 as u32 {
        MENU_SETTINGS => open_config(hwnd, state),
        MENU_RELOAD => reload_config(hwnd, state),
        MENU_LAST_ERROR => show_last_error(state.config.ui_language),
        MENU_LANGUAGE_ENGLISH => set_ui_language(hwnd, state, UiLanguage::English),
        MENU_LANGUAGE_CHINESE => set_ui_language(hwnd, state, UiLanguage::ChineseSimplified),
        MENU_EXIT => {
            let _ = DestroyWindow(hwnd);
        }
        _ => {}
    }
}

unsafe fn set_ui_language(hwnd: HWND, state: &mut AppState, language: UiLanguage) {
    if state.config.ui_language == language {
        return;
    }
    let previous = state.config.ui_language;
    state.config.ui_language = language;
    if let Err(error) = config::save(&state.config_path, &state.config) {
        state.config.ui_language = previous;
        notify(
            hwnd,
            state.icon,
            "TextPilot",
            &error.localized_message(previous),
            true,
        );
        return;
    }
    update_tooltip(hwnd, state.icon, &idle_tooltip_text(&state.config));
    if !state.settings_hwnd.is_invalid() && IsWindow(Some(state.settings_hwnd)).as_bool() {
        settings::set_locale(state.settings_hwnd, language);
    }
}

unsafe fn open_config(hwnd: HWND, state: &mut AppState) {
    if !state.settings_hwnd.is_invalid() && IsWindow(Some(state.settings_hwnd)).as_bool() {
        settings::focus(state.settings_hwnd);
        return;
    }
    let instance = HINSTANCE(match GetModuleHandleW(None) {
        Ok(module) => module.0,
        Err(error) => {
            notify(
                hwnd,
                state.icon,
                i18n::text(state.config.ui_language, Message::OpenSettingsFailed),
                &error.to_string(),
                true,
            );
            return;
        }
    });
    match settings::show(hwnd, instance, state.icon, &state.config) {
        Ok(settings_hwnd) => state.settings_hwnd = settings_hwnd,
        Err(error) => notify(
            hwnd,
            state.icon,
            i18n::text(state.config.ui_language, Message::OpenSettingsFailed),
            &error.to_string(),
            true,
        ),
    }
}

unsafe fn reload_config(hwnd: HWND, state: &mut AppState) {
    if state.busy {
        notify(
            hwnd,
            state.icon,
            "TextPilot",
            i18n::text(state.config.ui_language, Message::ReloadBusy),
            false,
        );
        return;
    }
    let new_config = match config::load_existing(&state.config_path) {
        Ok(config) => config,
        Err(error) => {
            notify(
                hwnd,
                state.icon,
                i18n::text(state.config.ui_language, Message::ReloadFailed),
                &error.localized_message(state.config.ui_language),
                true,
            );
            return;
        }
    };
    match apply_config(hwnd, state, new_config, false) {
        Ok(()) => {
            if !state.settings_hwnd.is_invalid() && IsWindow(Some(state.settings_hwnd)).as_bool() {
                settings::refresh(state.settings_hwnd, &state.config);
            }
            notify(
                hwnd,
                state.icon,
                "TextPilot",
                i18n::text(state.config.ui_language, Message::Reloaded),
                false,
            );
        }
        Err(error) => notify(
            hwnd,
            state.icon,
            i18n::text(state.config.ui_language, Message::ReloadFailed),
            &error,
            true,
        ),
    }
}

unsafe fn apply_config(
    hwnd: HWND,
    state: &mut AppState,
    new_config: Config,
    persist: bool,
) -> Result<(), String> {
    if state.busy {
        return Err(i18n::text(state.config.ui_language, Message::ApplyBusy).into());
    }
    new_config
        .validate()
        .map_err(|error| error.localized_message(new_config.ui_language))?;

    let old_config = state.config.clone();
    let old_registered = state.hotkeys_registered;
    let old_auto_start = state.config.auto_start;

    if old_registered {
        deactivate_hotkeys(hwnd, &old_config);
    }
    if let Err(error) = activate_hotkeys(hwnd, &new_config) {
        if old_registered {
            let _ = activate_hotkeys(hwnd, &old_config);
        }
        state.hotkeys_registered = old_registered;
        return Err(format!(
            "{}: {error}",
            i18n::text(new_config.ui_language, Message::HotkeyRegistrationFailed)
        ));
    }

    if let Err(error) = startup::set_auto_start(new_config.auto_start, &state.exe_path) {
        deactivate_hotkeys(hwnd, &new_config);
        if old_registered {
            let _ = activate_hotkeys(hwnd, &old_config);
        }
        let _ = startup::set_auto_start(old_auto_start, &state.exe_path);
        return Err(format!(
            "{}: {error}",
            i18n::text(new_config.ui_language, Message::AutoStartFailed)
        ));
    }

    if persist {
        if let Err(error) = config::save(&state.config_path, &new_config) {
            deactivate_hotkeys(hwnd, &new_config);
            if old_registered {
                let _ = activate_hotkeys(hwnd, &old_config);
            }
            let _ = startup::set_auto_start(old_auto_start, &state.exe_path);
            return Err(error.localized_message(new_config.ui_language));
        }
    }

    state.config = new_config;
    state.hotkeys_registered = true;
    update_tooltip(hwnd, state.icon, &idle_tooltip_text(&state.config));
    Ok(())
}

fn start_worker(
    command_rx: Receiver<WorkerCommand>,
    result_tx: Sender<WorkerResult>,
    hwnd_raw: isize,
) {
    thread::spawn(move || {
        let client = ApiClient::new();
        while let Ok(command) = command_rx.recv() {
            match command {
                WorkerCommand::ExecuteAction {
                    task_id,
                    action,
                    tier,
                    config,
                    text,
                } => {
                    let action_name = action.name.clone();
                    let result = recover_worker_operation(config.ui_language, || {
                        client
                            .execute_action_request(&config, &action, tier, &text, task_id)
                            .map_err(|error| error.localized_message(config.ui_language))
                    });
                    if result_tx
                        .send(WorkerResult::ExecuteAction {
                            task_id,
                            action_name,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                    unsafe {
                        let hwnd = HWND(hwnd_raw as *mut c_void);
                        let _ = PostMessageW(Some(hwnd), WM_WORKER_DONE, WPARAM(0), LPARAM(0));
                    }
                }
                WorkerCommand::TestApi {
                    settings_hwnd,
                    config,
                } => {
                    let result = recover_worker_operation(config.ui_language, || {
                        client
                            .test_connection(&config)
                            .map_err(|error| error.localized_message(config.ui_language))
                    });
                    if result_tx
                        .send(WorkerResult::TestApi {
                            settings_hwnd,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                    unsafe {
                        let hwnd = HWND(hwnd_raw as *mut c_void);
                        let _ = PostMessageW(Some(hwnd), WM_WORKER_DONE, WPARAM(0), LPARAM(0));
                    }
                }
                WorkerCommand::TestTranslationApi {
                    settings_hwnd,
                    config,
                } => {
                    let result = recover_worker_operation(config.ui_language, || {
                        client
                            .test_translation_connection(&config)
                            .map_err(|error| error.localized_message(config.ui_language))
                    });
                    if result_tx
                        .send(WorkerResult::TestTranslationApi {
                            settings_hwnd,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                    unsafe {
                        let hwnd = HWND(hwnd_raw as *mut c_void);
                        let _ = PostMessageW(Some(hwnd), WM_WORKER_DONE, WPARAM(0), LPARAM(0));
                    }
                }
                WorkerCommand::TestActionApi {
                    settings_hwnd,
                    action,
                    config,
                } => {
                    let action_name = action.name.clone();
                    let result = recover_worker_operation(config.ui_language, || {
                        client
                            .test_action_connection(&config, &action)
                            .map_err(|error| error.localized_message(config.ui_language))
                    });
                    if result_tx
                        .send(WorkerResult::TestActionApi {
                            settings_hwnd,
                            action_name,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                    unsafe {
                        let hwnd = HWND(hwnd_raw as *mut c_void);
                        let _ = PostMessageW(Some(hwnd), WM_WORKER_DONE, WPARAM(0), LPARAM(0));
                    }
                }
                WorkerCommand::Shutdown => break,
            }
        }
    });
}

fn recover_worker_operation<Value, Operation>(
    language: UiLanguage,
    operation: Operation,
) -> Result<Value, String>
where
    Operation: FnOnce() -> Result<Value, String>,
{
    catch_unwind(AssertUnwindSafe(operation))
        .unwrap_or_else(|_| Err(i18n::text(language, Message::WorkerRecovered).into()))
}

fn activate_hotkeys(hwnd: HWND, config: &Config) -> Result<(), WindowsError> {
    let result = (|| {
        for (index, action) in config.actions.iter().enumerate() {
            if !action.enabled {
                continue;
            }
            let Ok(spec) = parse_hotkey(&action.hotkey) else {
                continue;
            };
            match spec.kind {
                HotkeyKind::Chord {
                    modifiers,
                    virtual_key,
                } => unsafe {
                    let id = BASE_HOTKEY_ID + index as i32;
                    RegisterHotKey(Some(hwnd), id, HOT_KEY_MODIFIERS(modifiers), virtual_key)?;
                },
                HotkeyKind::CtrlMultiTap { taps, virtual_key } => {
                    install_gesture_hook_slot(
                        hwnd,
                        index as u32,
                        virtual_key,
                        taps,
                        !action.triple_prompt.trim().is_empty(),
                    )?;
                }
            }
        }
        Ok(())
    })();
    if result.is_err() {
        deactivate_hotkeys(hwnd, config);
    }
    result
}

fn deactivate_hotkeys(hwnd: HWND, config: &Config) {
    for (index, action) in config.actions.iter().enumerate() {
        if let Ok(spec) = parse_hotkey(&action.hotkey) {
            if let HotkeyKind::Chord { .. } = spec.kind {
                unsafe {
                    let _ = UnregisterHotKey(Some(hwnd), BASE_HOTKEY_ID + index as i32);
                }
            }
        }
    }
    uninstall_all_gesture_hook_slots(hwnd);
}

fn install_gesture_hook_slot(
    hwnd: HWND,
    action_index: u32,
    virtual_key: u32,
    taps: u8,
    has_deep_tier: bool,
) -> Result<(), WindowsError> {
    let mut state = GESTURE_HOOK
        .lock()
        .map_err(|_| WindowsError::from_thread())?;
    if state.hook == 0 {
        let module = HINSTANCE(unsafe { GetModuleHandleW(None) }?.0);
        let hook = unsafe {
            SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), Some(module), 0)
        }?;
        state.hook = hook.0 as isize;
        state.hwnd = hwnd.0 as isize;
    }
    let slot = if let Some(index) = state
        .slots
        .iter()
        .position(|slot| slot.virtual_key == virtual_key)
    {
        &mut state.slots[index]
    } else {
        state.slots.push(GestureSlot {
            virtual_key,
            ..Default::default()
        });
        state.slots.last_mut().expect("gesture slot was inserted")
    };
    if taps == 3 {
        slot.triple_action_index = Some(action_index);
    } else {
        slot.double_action_index = Some(action_index);
        slot.double_has_deep_tier = has_deep_tier;
    }
    Ok(())
}

fn uninstall_all_gesture_hook_slots(hwnd: HWND) {
    let hook_to_unhook = if let Ok(mut state) = GESTURE_HOOK.lock() {
        state.slots.clear();
        let hook = state.hook;
        state.hook = 0;
        state.hwnd = 0;
        hook
    } else {
        0
    };
    if hook_to_unhook != 0 {
        unsafe {
            let _ = KillTimer(Some(hwnd), GESTURE_TIMER_ID);
            let _ = UnhookWindowsHookEx(HHOOK(hook_to_unhook as *mut c_void));
        }
    }
}

unsafe extern "system" fn keyboard_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 || lparam.0 == 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }
    let event = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
    if event.flags.0 & LLKHF_INJECTED.0 != 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let message = wparam.0 as u32;
    let key_down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
    let key_up = message == WM_KEYUP || message == WM_SYSKEYUP;
    let is_ctrl = matches!(event.vkCode, 0x11 | 0xA2 | 0xA3);
    let ctrl_down = GetAsyncKeyState(VK_CONTROL.0 as i32) < 0;
    let mut replays = Vec::new();
    let mut trigger_hwnd = 0;
    let mut trigger_action_index = 0;
    let mut trigger_tier = ActionTier::Standard;
    let mut suppress = false;
    let mut timer_should_stop = false;

    if let Ok(mut state) = GESTURE_HOOK.lock() {
        if state.hook != 0 {
            let state_hwnd = state.hwnd;
            let slot_index = state
                .slots
                .iter()
                .position(|slot| slot.virtual_key == event.vkCode);
            if let Some(slot_index) = slot_index {
                let virtual_key = state.slots[slot_index].virtual_key;
                if key_down && ctrl_down {
                    let (prev_trigger, prev_replays) =
                        process_pending_gestures(&mut state.slots, Some(virtual_key));
                    if let Some((act_idx, tier)) = prev_trigger {
                        trigger_hwnd = state_hwnd;
                        trigger_action_index = act_idx;
                        trigger_tier = tier;
                    }
                    replays.extend(prev_replays);
                    let next_sequence = state.next_sequence;
                    let mut consume_sequence = false;
                    let slot = &mut state.slots[slot_index];
                    if !slot.key_down {
                        let (taps, expired_replay, triggered_tier) = advance_gesture_tap(
                            slot.taps,
                            slot.last_tap_time,
                            event.time,
                            slot.has_triple_target(),
                        );
                        slot.taps = taps;
                        if expired_replay > 0 {
                            replays.push((expired_replay, slot.virtual_key));
                            slot.sequence = 0;
                        }
                        if taps == 1 {
                            slot.sequence = next_sequence;
                            consume_sequence = true;
                        }
                        slot.key_down = true;
                        slot.last_tap_time = event.time;
                        let hwnd = HWND(state_hwnd as *mut c_void);
                        let _ = KillTimer(Some(hwnd), GESTURE_TIMER_ID);

                        if let Some(tier) = triggered_tier {
                            if let Some((action_index, action_tier)) = slot.action_for_tier(tier) {
                                trigger_hwnd = state_hwnd;
                                trigger_action_index = action_index;
                                trigger_tier = action_tier;
                            } else {
                                replays.push((
                                    if tier == ActionTier::Deep { 3 } else { 2 },
                                    slot.virtual_key,
                                ));
                            }
                            slot.sequence = 0;
                            slot.taps = 0;
                        } else if taps == 2 {
                            // Waiting window for potential triple tap
                            SetTimer(Some(hwnd), GESTURE_TIMER_ID, TIER_DISPATCH_WINDOW_MS, None);
                        } else if taps == 1 {
                            SetTimer(Some(hwnd), GESTURE_TIMER_ID, GESTURE_INTERVAL_MS, None);
                        }
                    }
                    if consume_sequence {
                        state.next_sequence = state.next_sequence.wrapping_add(1).max(1);
                    }
                    suppress = true;
                } else if key_up && state.slots[slot_index].key_down {
                    state.slots[slot_index].key_down = false;
                    suppress = true;
                }
            } else if (is_ctrl && key_up) || key_down {
                let (pending_trigger, pending_replays) =
                    process_pending_gestures(&mut state.slots, None);
                if let Some((act_idx, tier)) = pending_trigger {
                    trigger_hwnd = state_hwnd;
                    trigger_action_index = act_idx;
                    trigger_tier = tier;
                }
                replays.extend(pending_replays);
                timer_should_stop = !replays.is_empty() || trigger_hwnd != 0;
            }
        }
    }

    if timer_should_stop && trigger_hwnd == 0 {
        if let Ok(state) = GESTURE_HOOK.lock() {
            if state.hwnd != 0 {
                let _ = KillTimer(Some(HWND(state.hwnd as *mut c_void)), GESTURE_TIMER_ID);
            }
        }
    }
    for (replay, replay_key) in replays {
        let _ = clipboard::replay_ctrl_key(replay_key, replay);
    }
    if trigger_hwnd != 0 {
        let _ = PostMessageW(
            Some(HWND(trigger_hwnd as *mut c_void)),
            WM_GESTURE_HOTKEY,
            WPARAM(trigger_action_index as usize),
            LPARAM(if trigger_tier == ActionTier::Deep {
                1
            } else {
                0
            }),
        );
    }
    if suppress {
        LRESULT(1)
    } else {
        CallNextHookEx(None, code, wparam, lparam)
    }
}

fn handle_gesture_timer(hwnd: HWND) {
    unsafe {
        let _ = KillTimer(Some(hwnd), GESTURE_TIMER_ID);
    }
    let (triggered, to_replay) = if let Ok(mut state) = GESTURE_HOOK.lock() {
        process_pending_gestures(&mut state.slots, None)
    } else {
        (None, Vec::new())
    };

    if let Some((action_index, tier)) = triggered {
        unsafe {
            let _ = PostMessageW(
                Some(hwnd),
                WM_GESTURE_HOTKEY,
                WPARAM(action_index as usize),
                LPARAM(if tier == ActionTier::Deep { 1 } else { 0 }),
            );
        }
    }

    for (replay, virtual_key) in to_replay {
        let _ = clipboard::replay_ctrl_key(virtual_key, replay);
    }
}

fn add_tray_icon(hwnd: HWND, state: &AppState) -> Result<(), AppError> {
    let mut data = tray_data(hwnd, state.icon);
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    copy_wide(&mut data.szTip, &idle_tooltip_text(&state.config));
    if !unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
        return Err(AppError(WindowsError::from_thread().to_string()));
    }
    Ok(())
}

fn delete_tray_icon(hwnd: HWND) {
    let data = tray_data(hwnd, HICON::default());
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

fn update_tooltip(hwnd: HWND, icon: HICON, text: &str) {
    let mut data = tray_data(hwnd, icon);
    data.uFlags = NIF_TIP;
    copy_wide(&mut data.szTip, text);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

fn notify(hwnd: HWND, icon: HICON, title: &str, message: &str, is_error: bool) {
    let _ = (hwnd, icon);
    if is_error {
        remember_last_error(title, message);
    }
    show_status_popup(
        message,
        if is_error {
            StatusKind::Error
        } else {
            StatusKind::Neutral
        },
        false,
    );
}

fn remember_last_error(title: &str, message: &str) {
    if let Ok(mut last_error) = LAST_ERROR.lock() {
        *last_error = Some((title.to_string(), message.to_string()));
    }
}

unsafe fn show_last_error(language: UiLanguage) {
    let error = LAST_ERROR.lock().ok().and_then(|error| error.clone());
    let Some((title, message)) = error else {
        return;
    };
    let content = wide(&format!("{title}\n\n{message}"));
    let dialog_title = wide(i18n::text(language, Message::LastErrorTitle));
    MessageBoxW(
        None,
        PCWSTR(content.as_ptr()),
        PCWSTR(dialog_title.as_ptr()),
        MB_OK | MB_ICONERROR,
    );
}

fn show_status_popup(message: &str, kind: StatusKind, new_task: bool) {
    let concise: String = message.chars().take(80).collect();
    let dpi = popup_dpi();
    let width = status_text_width(&concise, dpi);
    let height = popup_scale(STATUS_HEIGHT, dpi);
    let corner_radius = popup_scale(10, dpi);
    let text = wide(&concise);

    let existing_hwnd = match STATUS_POPUP.lock() {
        Ok(state) => state.hwnd,
        Err(_) => return,
    };
    let popup = if existing_hwnd == 0 {
        let Ok(module) = (unsafe { GetModuleHandleW(None) }) else {
            return;
        };
        let created = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(
                    WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0 | WS_EX_LAYERED.0,
                ),
                STATUS_WINDOW_CLASS,
                PCWSTR(text.as_ptr()),
                WS_POPUP,
                0,
                0,
                width,
                height,
                None,
                None,
                Some(HINSTANCE(module.0)),
                None,
            )
        };
        let Ok(created) = created else {
            return;
        };
        unsafe {
            let _ = SetLayeredWindowAttributes(created, COLORREF(0), 206, LWA_ALPHA);
        }
        let Ok(mut state) = STATUS_POPUP.lock() else {
            unsafe {
                let _ = DestroyWindow(created);
            }
            return;
        };
        state.hwnd = created.0 as isize;
        created
    } else {
        HWND(existing_hwnd as *mut c_void)
    };

    let should_capture_anchor = match STATUS_POPUP.lock() {
        Ok(state) => new_task || !state.visible || state.anchor.is_none(),
        Err(_) => return,
    };
    let captured_anchor = should_capture_anchor.then(|| capture_popup_anchor(dpi));
    let anchor = match STATUS_POPUP.lock() {
        Ok(mut state) => {
            if let Some(anchor) = captured_anchor {
                state.anchor = Some(anchor);
            }
            state.kind = kind;
            match state.anchor {
                Some(anchor) => anchor,
                None => return,
            }
        }
        Err(_) => return,
    };
    let display_width = width.min(anchor.max_width);
    let (x, y) = anchor.position(display_width);
    unsafe {
        let _ = SetWindowTextW(popup, PCWSTR(text.as_ptr()));
        let region = CreateRoundRectRgn(
            0,
            0,
            display_width + 1,
            height + 1,
            corner_radius,
            corner_radius,
        );
        let _ = SetWindowRgn(popup, Some(region), false);
        let _ = SetWindowPos(
            popup,
            Some(HWND_TOPMOST),
            x,
            y,
            display_width,
            height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        let _ = InvalidateRect(Some(popup), None, false);
        let _ = KillTimer(Some(popup), 1);
        let keep_open = new_task || kind == StatusKind::Progress;
        if !keep_open {
            let duration = if kind == StatusKind::Error {
                2400
            } else {
                1200
            };
            SetTimer(Some(popup), 1, duration, Some(status_popup_timer));
        }
    }
    if let Ok(mut state) = STATUS_POPUP.lock() {
        if state.hwnd == popup.0 as isize {
            state.visible = true;
        }
    }
}

fn status_text_width(message: &str, dpi: u32) -> i32 {
    let text_width: i32 = message
        .chars()
        .map(|character| if character.is_ascii() { 7 } else { 14 })
        .sum();
    popup_scale(text_width + 38, dpi).clamp(
        popup_scale(STATUS_MIN_WIDTH, dpi),
        popup_scale(STATUS_MAX_WIDTH, dpi),
    )
}

fn popup_dpi() -> u32 {
    unsafe { GetDpiForWindow(GetForegroundWindow()).max(96) }
}

fn popup_scale(value: i32, dpi: u32) -> i32 {
    ((value as i64 * dpi as i64 + 48) / 96) as i32
}

fn capture_popup_anchor(dpi: u32) -> PopupAnchor {
    let height = popup_scale(STATUS_HEIGHT, dpi);
    let gap = popup_scale(12, dpi);
    let anchor = selection::take_selection_rect().unwrap_or_else(|| {
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
            left: anchor.left - 1000,
            top: anchor.top - 1000,
            right: anchor.right + 1000,
            bottom: anchor.bottom + 1000,
        }
    };

    let right_edge = anchor.right + gap;
    let left_edge = anchor.left - gap;
    let right_space = work.right - right_edge;
    let left_space = left_edge - work.left;
    let (side, edge_x, available) = if right_space >= left_space {
        (PopupSide::Right, right_edge, right_space)
    } else {
        (PopupSide::Left, left_edge, left_space)
    };
    let mut y = anchor.bottom + gap;
    if y + height > work.bottom {
        y = anchor.top - height - gap;
    }
    y = y.clamp(work.top, (work.bottom - height).max(work.top));
    PopupAnchor {
        side,
        edge_x,
        y,
        max_width: available.clamp(
            popup_scale(STATUS_MIN_WIDTH, dpi),
            popup_scale(STATUS_MAX_WIDTH, dpi),
        ),
        work_left: work.left,
        work_right: work.right,
    }
}

unsafe extern "system" fn status_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_PAINT => {
            paint_status_window(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe fn create_status_popup_font(dpi: u32) -> HFONT {
    windows::Win32::Graphics::Gdi::CreateFontW(
        -popup_scale(13, dpi),
        0,
        0,
        0,
        500,
        0,
        0,
        0,
        windows::Win32::Graphics::Gdi::DEFAULT_CHARSET,
        windows::Win32::Graphics::Gdi::FONT_OUTPUT_PRECISION::default(),
        windows::Win32::Graphics::Gdi::FONT_CLIP_PRECISION::default(),
        windows::Win32::Graphics::Gdi::FONT_QUALITY(5),
        windows::Win32::Graphics::Gdi::FF_DONTCARE.0 as u32,
        w!("Segoe UI Variable Text"),
    )
}

unsafe fn paint_status_window(hwnd: HWND) {
    let mut paint = PAINTSTRUCT::default();
    let dc = BeginPaint(hwnd, &mut paint);
    let mut rect = RECT::default();
    let _ = GetClientRect(hwnd, &mut rect);
    let dpi = GetDpiForWindow(hwnd).max(96);
    let corner_radius = popup_scale(10, dpi);

    let bg_color = COLORREF(0x00FF_FFFF);
    let border_color = COLORREF(0x00EB_E7E5);
    let brush = CreateSolidBrush(bg_color);
    let pen = CreatePen(windows::Win32::Graphics::Gdi::PS_SOLID, 1, border_color);

    let old_brush = SelectObject(dc, brush.into());
    let old_pen = SelectObject(dc, pen.into());

    let _ = RoundRect(
        dc,
        0,
        0,
        rect.right,
        rect.bottom,
        corner_radius,
        corner_radius,
    );

    let _ = SelectObject(dc, old_pen);
    let _ = SelectObject(dc, old_brush);
    let _ = DeleteObject(pen.into());
    let _ = DeleteObject(brush.into());

    let font = create_status_popup_font(dpi);
    let previous_font = SelectObject(dc, font.into());

    let _ = SetBkMode(dc, TRANSPARENT);

    let mut text_buf = [0_u16; 96];
    let length = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(hwnd, &mut text_buf);

    let kind = STATUS_POPUP
        .lock()
        .map(|state| state.kind)
        .unwrap_or(StatusKind::Neutral);
    let dot_color = match kind {
        StatusKind::Progress => COLORREF(0x00EB_6325),
        StatusKind::Success => COLORREF(0x0081_B910),
        StatusKind::Error => COLORREF(0x0044_44EF),
        StatusKind::Neutral => COLORREF(0x0080_7464),
    };

    let dot_brush = CreateSolidBrush(dot_color);
    let dot_pen = CreatePen(windows::Win32::Graphics::Gdi::PS_SOLID, 1, dot_color);
    let old_dbrush = SelectObject(dc, dot_brush.into());
    let old_dpen = SelectObject(dc, dot_pen.into());

    let dot_size = popup_scale(7, dpi);
    let dot_top = (rect.bottom - dot_size) / 2;
    let dot_left = popup_scale(12, dpi);
    let _ = RoundRect(
        dc,
        dot_left,
        dot_top,
        dot_left + dot_size,
        dot_top + dot_size,
        dot_size,
        dot_size,
    );

    let _ = SelectObject(dc, old_dpen);
    let _ = SelectObject(dc, old_dbrush);
    let _ = DeleteObject(dot_pen.into());
    let _ = DeleteObject(dot_brush.into());

    let _ = SetTextColor(dc, COLORREF(0x002A_170F));
    let mut text_rect = RECT {
        left: popup_scale(26, dpi),
        top: 0,
        right: rect.right - popup_scale(10, dpi),
        bottom: rect.bottom,
    };
    DrawTextW(
        dc,
        &mut text_buf[..length.max(0) as usize],
        &mut text_rect,
        windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT(
            windows::Win32::Graphics::Gdi::DT_LEFT.0
                | DT_END_ELLIPSIS.0
                | DT_SINGLELINE.0
                | DT_VCENTER.0,
        ),
    );

    let _ = SelectObject(dc, previous_font);
    let _ = DeleteObject(font.into());
    let _ = EndPaint(hwnd, &paint);
}

unsafe extern "system" fn status_popup_timer(hwnd: HWND, _: u32, _: usize, _: u32) {
    let _ = KillTimer(Some(hwnd), 1);
    let _ = ShowWindow(hwnd, SW_HIDE);
    if let Ok(mut state) = STATUS_POPUP.lock() {
        if state.hwnd == hwnd.0 as isize {
            state.visible = false;
        }
    }
}

fn tray_data(hwnd: HWND, icon: HICON) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        hIcon: icon,
        ..Default::default()
    }
}

fn create_embedded_icon(size: i32) -> Result<HICON, AppError> {
    let resource = ico_image_nearest_to(ICON_FILE, size as u32).ok_or_else(|| {
        AppError(i18n::text(UiLanguage::ChineseSimplified, Message::InvalidEmbeddedIcon).into())
    })?;
    unsafe {
        CreateIconFromResourceEx(
            resource,
            true,
            0x0003_0000,
            size,
            size,
            IMAGE_FLAGS(LR_DEFAULTCOLOR.0),
        )
    }
    .map_err(AppError::from)
}

fn ico_image_nearest_to(bytes: &[u8], desired: u32) -> Option<&[u8]> {
    if bytes.len() < 6 || bytes[0..4] != [0, 0, 1, 0] {
        return None;
    }
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    (0..count)
        .filter_map(|index| {
            let entry = 6 + index * 16;
            let data = bytes.get(entry..entry + 16)?;
            let width = if data[0] == 0 { 256 } else { data[0] as u32 };
            let length = u32::from_le_bytes(data[8..12].try_into().ok()?) as usize;
            let offset = u32::from_le_bytes(data[12..16].try_into().ok()?) as usize;
            let resource = bytes.get(offset..offset.checked_add(length)?)?;
            Some((width.abs_diff(desired), resource))
        })
        .min_by_key(|(difference, _)| *difference)
        .map(|(_, resource)| resource)
}

fn copy_wide<const N: usize>(destination: &mut [u16; N], text: &str) {
    destination.fill(0);
    for (slot, value) in destination
        .iter_mut()
        .take(N.saturating_sub(1))
        .zip(text.encode_utf16())
    {
        *slot = value;
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn show_fatal_error(message: &str) {
    let wide_message = wide(message);
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(wide_message.as_ptr()),
            APP_NAME,
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(test)]
mod status_popup_tests {
    use super::*;

    #[test]
    fn status_popup_paints_without_blocking_the_ui_thread() {
        use std::sync::mpsc;
        use std::time::Duration;

        if let Ok(mut state) = STATUS_POPUP.lock() {
            *state = StatusPopupState {
                hwnd: 0,
                visible: false,
                anchor: None,
                kind: StatusKind::Neutral,
            };
        }

        let (result_tx, result_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = unsafe {
                let instance = match GetModuleHandleW(None) {
                    Ok(module) => HINSTANCE(module.0),
                    Err(_) => {
                        let _ = result_tx.send(false);
                        return;
                    }
                };
                let class = WNDCLASSW {
                    hInstance: instance,
                    lpszClassName: STATUS_WINDOW_CLASS,
                    lpfnWndProc: Some(status_window_proc),
                    ..Default::default()
                };
                if RegisterClassW(&class) == 0 {
                    let _ = result_tx.send(false);
                    return;
                }

                show_status_popup("Processing regression test", StatusKind::Progress, true);
                let popup = STATUS_POPUP
                    .lock()
                    .ok()
                    .map(|state| HWND(state.hwnd as *mut c_void))
                    .unwrap_or_default();
                if popup.is_invalid() {
                    false
                } else {
                    let _ = UpdateWindow(popup);
                    let mut text = [0_u16; 96];
                    let length =
                        windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(popup, &mut text);
                    let _ = DestroyWindow(popup);
                    length > 0
                }
            };

            if let Ok(mut state) = STATUS_POPUP.lock() {
                state.hwnd = 0;
                state.visible = false;
                state.anchor = None;
            }
            let _ = result_tx.send(result);
        });

        assert_eq!(
            result_rx.recv_timeout(Duration::from_secs(2)),
            Ok(true),
            "状态框显示或 WM_PAINT 被互斥锁重入阻塞"
        );
    }

    #[test]
    fn width_stays_within_compact_limits() {
        assert_eq!(status_text_width("已复制", 96), STATUS_MIN_WIDTH);
        assert_eq!(status_text_width(&"错误".repeat(40), 96), STATUS_MAX_WIDTH);
    }

    #[test]
    fn full_error_detail_remains_available_after_compact_notification() {
        let detail = "provider detail".repeat(40);
        remember_last_error("API failed", &detail);

        let stored = LAST_ERROR.lock().unwrap().clone();
        assert_eq!(stored, Some(("API failed".into(), detail)));
    }

    #[test]
    fn dynamic_width_keeps_the_anchor_edge_fixed() {
        let right = PopupAnchor {
            side: PopupSide::Right,
            edge_x: 100,
            y: 50,
            max_width: 300,
            work_left: 0,
            work_right: 800,
        };
        assert_eq!(right.position(112), (100, 50));
        assert_eq!(right.position(220), (100, 50));

        let left = PopupAnchor {
            side: PopupSide::Left,
            edge_x: 500,
            y: 50,
            max_width: 300,
            work_left: 0,
            work_right: 800,
        };
        assert_eq!(left.position(112).0 + 112, 500);
        assert_eq!(left.position(220).0 + 220, 500);
    }

    #[test]
    fn triple_tap_triggers_only_on_the_third_quick_press() {
        assert_eq!(advance_gesture_tap(0, 0, 100, true), (1, 0, None));
        assert_eq!(advance_gesture_tap(1, 100, 250, true), (2, 0, None));
        assert_eq!(
            advance_gesture_tap(2, 250, 400, true),
            (0, 0, Some(ActionTier::Deep))
        );
    }

    #[test]
    fn double_tap_without_triple_tier_triggers_immediately_on_second_press() {
        assert_eq!(advance_gesture_tap(0, 0, 100, false), (1, 0, None));
        assert_eq!(
            advance_gesture_tap(1, 100, 250, false),
            (0, 0, Some(ActionTier::Standard))
        );
    }

    #[test]
    fn expired_taps_are_replayed_before_starting_a_new_sequence() {
        assert_eq!(
            advance_gesture_tap(2, 100, 100 + GESTURE_INTERVAL_MS + 1, true),
            (1, 2, None)
        );
    }

    #[test]
    fn pending_gestures_replay_in_the_order_they_started() {
        let mut slots = vec![
            GestureSlot {
                double_action_index: Some(0),
                virtual_key: 0x77,
                double_has_deep_tier: true,
                taps: 1,
                key_down: false,
                last_tap_time: 200,
                sequence: 2,
                ..Default::default()
            },
            GestureSlot {
                double_action_index: Some(1),
                virtual_key: 0x78,
                double_has_deep_tier: true,
                taps: 1,
                key_down: false,
                last_tap_time: 100,
                sequence: 1,
                ..Default::default()
            },
        ];

        let (triggered, replays) = process_pending_gestures(&mut slots, None);
        assert_eq!(triggered, None);
        assert_eq!(replays, vec![(1, 0x78), (1, 0x77)]);
        assert!(slots
            .iter()
            .all(|slot| slot.taps == 0 && slot.sequence == 0));
    }

    #[test]
    fn switching_gesture_keys_drains_only_the_previous_sequence() {
        let mut slots = vec![
            GestureSlot {
                double_action_index: Some(0),
                virtual_key: 0x77,
                double_has_deep_tier: true,
                taps: 1,
                key_down: false,
                last_tap_time: 100,
                sequence: 1,
                ..Default::default()
            },
            GestureSlot {
                double_action_index: Some(1),
                virtual_key: 0x78,
                double_has_deep_tier: true,
                taps: 1,
                key_down: false,
                last_tap_time: 200,
                sequence: 2,
                ..Default::default()
            },
        ];

        let (triggered, replays) = process_pending_gestures(&mut slots, Some(0x78));
        assert_eq!(triggered, None);
        assert_eq!(replays, vec![(1, 0x77)]);
        assert_eq!(slots[0].taps, 0);
        assert_eq!(slots[1].taps, 1);
    }

    #[test]
    fn completed_double_tap_triggers_standard_tier_on_external_key_or_ctrl_release() {
        let mut slots = vec![GestureSlot {
            double_action_index: Some(0),
            virtual_key: 0x77,
            double_has_deep_tier: true,
            taps: 2,
            key_down: false,
            last_tap_time: 100,
            sequence: 1,
            ..Default::default()
        }];

        let (triggered, replays) = process_pending_gestures(&mut slots, None);
        assert_eq!(triggered, Some((0, ActionTier::Standard)));
        assert_eq!(replays, vec![]);
        assert_eq!(slots[0].taps, 0);
    }

    #[test]
    fn explicit_double_and_triple_actions_route_to_separate_targets() {
        let slot = GestureSlot {
            double_action_index: Some(3),
            triple_action_index: Some(7),
            virtual_key: 0x75,
            ..Default::default()
        };

        assert_eq!(
            slot.action_for_tier(ActionTier::Standard),
            Some((3, ActionTier::Standard))
        );
        assert_eq!(
            slot.action_for_tier(ActionTier::Deep),
            Some((7, ActionTier::Deep))
        );
        assert!(slot.has_triple_target());
    }

    #[test]
    fn double_action_deep_prompt_is_the_fallback_triple_target() {
        let slot = GestureSlot {
            double_action_index: Some(3),
            virtual_key: 0x75,
            double_has_deep_tier: true,
            ..Default::default()
        };

        assert_eq!(
            slot.action_for_tier(ActionTier::Deep),
            Some((3, ActionTier::Deep))
        );
    }

    #[test]
    fn triple_only_slot_does_not_dispatch_on_two_taps() {
        let mut slots = vec![GestureSlot {
            triple_action_index: Some(7),
            virtual_key: 0x75,
            taps: 2,
            sequence: 1,
            ..Default::default()
        }];

        let (triggered, replays) = process_pending_gestures(&mut slots, None);
        assert_eq!(triggered, None);
        assert_eq!(replays, vec![(2, 0x75)]);
    }

    #[test]
    fn embedded_ico_contains_small_and_large_icon_images() {
        assert!(ico_image_nearest_to(ICON_FILE, 16).is_some());
        assert!(ico_image_nearest_to(ICON_FILE, 256).is_some());
        assert!(ico_image_nearest_to(b"not-an-icon", 16).is_none());
    }

    #[test]
    fn worker_panic_is_converted_to_a_recoverable_error() {
        let result =
            recover_worker_operation(UiLanguage::ChineseSimplified, || -> Result<(), String> {
                panic!("simulated worker failure")
            });

        assert_eq!(
            result,
            Err("后台任务发生异常，已恢复，可重新触发快捷键".into())
        );
        assert_eq!(
            recover_worker_operation(UiLanguage::ChineseSimplified, || Ok(42)),
            Ok(42)
        );
    }
}
