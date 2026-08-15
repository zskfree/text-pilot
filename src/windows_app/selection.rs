use std::fmt::{Display, Formatter};
use std::sync::Mutex;
use text_pilot::config::UiLanguage;
use windows::core::Error as WindowsError;
use windows::Win32::Foundation::RECT;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};

static LAST_SELECTION_RECT: Mutex<Option<RECT>> = Mutex::new(None);

pub fn take_selection_rect() -> Option<RECT> {
    LAST_SELECTION_RECT
        .lock()
        .ok()
        .and_then(|mut rect| rect.take())
}

fn clear_selection_rect() {
    if let Ok(mut rect) = LAST_SELECTION_RECT.lock() {
        *rect = None;
    }
}

fn remember_selection_rect(rect: RECT) {
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width > 0 && height > 0 && width <= 800 && height <= 400 {
        if let Ok(mut last) = LAST_SELECTION_RECT.lock() {
            *last = Some(rect);
        }
    }
}
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationTextPattern, IUIAutomationTextPattern2,
    UIA_TextPattern2Id, UIA_TextPatternId,
};

#[derive(Debug)]
pub enum SelectionError {
    Unsupported,
    Windows(WindowsError),
    Compatibility(WindowsError),
}

impl Display for SelectionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.localized_message(UiLanguage::ChineseSimplified))
    }
}

impl SelectionError {
    pub fn localized_message(&self, language: UiLanguage) -> String {
        match self {
            Self::Unsupported => match language {
                UiLanguage::English => {
                    "The active application does not expose the selected text".into()
                }
                UiLanguage::ChineseSimplified => "当前应用不支持直接读取选中文本".into(),
            },
            Self::Windows(error) => match language {
                UiLanguage::English => format!("Windows UI Automation failed: {error}"),
                UiLanguage::ChineseSimplified => {
                    format!("Windows UI Automation 错误：{error}")
                }
            },
            Self::Compatibility(error) => match language {
                UiLanguage::English => {
                    format!("Clipboard compatibility selection failed: {error}")
                }
                UiLanguage::ChineseSimplified => format!("兼容模式读取选区失败：{error}"),
            },
        }
    }
}

impl std::error::Error for SelectionError {}

impl From<WindowsError> for SelectionError {
    fn from(value: WindowsError) -> Self {
        Self::Windows(value)
    }
}

struct ComGuard;

impl ComGuard {
    fn initialize() -> Result<Self, SelectionError> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
        Ok(Self)
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

pub fn read_selected_text() -> Result<Option<String>, SelectionError> {
    clear_selection_rect();
    let _com = ComGuard::initialize()?;
    read_with_compatibility(read_selected_text_via_uia, || {
        super::clipboard::read_selected_text_compatibility().map_err(SelectionError::Compatibility)
    })
}

fn read_selected_text_via_uia() -> Result<Option<String>, SelectionError> {
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }?;
    let element = unsafe { automation.GetFocusedElement() }?;

    if let Ok(rect) = unsafe { element.CurrentBoundingRectangle() } {
        remember_selection_rect(rect);
    }

    let texts = if let Ok(pattern) =
        unsafe { element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }
    {
        selected_ranges(&pattern)?
    } else if let Ok(pattern) =
        unsafe { element.GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id) }
    {
        selected_ranges(&pattern)?
    } else {
        return Err(SelectionError::Unsupported);
    };

    Ok(combine_selected_ranges(texts))
}

fn read_with_compatibility<Primary, Fallback>(
    primary: Primary,
    fallback: Fallback,
) -> Result<Option<String>, SelectionError>
where
    Primary: FnOnce() -> Result<Option<String>, SelectionError>,
    Fallback: FnOnce() -> Result<Option<String>, SelectionError>,
{
    match primary() {
        Ok(Some(text)) => Ok(Some(text)),
        Ok(None) | Err(_) => fallback(),
    }
}

fn selected_ranges(pattern: &IUIAutomationTextPattern) -> Result<Vec<String>, SelectionError> {
    let ranges = unsafe { pattern.GetSelection() }?;
    let length = unsafe { ranges.Length() }?;
    let mut texts = Vec::with_capacity(length.max(0) as usize);
    for index in 0..length {
        let range = unsafe { ranges.GetElement(index) }?;
        let text = unsafe { range.GetText(-1) }?.to_string();
        texts.push(text);
    }
    Ok(texts)
}

fn combine_selected_ranges(texts: Vec<String>) -> Option<String> {
    let text = texts
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    (!text.trim().is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn combines_multiple_nonempty_selection_ranges() {
        assert_eq!(
            combine_selected_ranges(vec!["第一段".into(), String::new(), "第二段".into()]),
            Some("第一段\n第二段".into())
        );
    }

    #[test]
    fn rejects_empty_or_whitespace_only_selection() {
        assert_eq!(combine_selected_ranges(vec![]), None);
        assert_eq!(combine_selected_ranges(vec!["  \r\n".into()]), None);
    }

    #[test]
    fn selection_errors_follow_the_interface_language() {
        assert_eq!(
            SelectionError::Unsupported.localized_message(UiLanguage::English),
            "The active application does not expose the selected text"
        );
        assert_eq!(
            SelectionError::Unsupported.localized_message(UiLanguage::ChineseSimplified),
            "当前应用不支持直接读取选中文本"
        );
    }

    #[test]
    fn uia_failure_automatically_uses_compatibility_reader() {
        let result = read_with_compatibility(
            || Err(SelectionError::Unsupported),
            || Ok(Some("来自兼容模式".into())),
        )
        .unwrap();

        assert_eq!(result.as_deref(), Some("来自兼容模式"));
    }

    #[test]
    fn uia_empty_selection_automatically_uses_compatibility_reader() {
        let result =
            read_with_compatibility(|| Ok(None), || Ok(Some("AntiGravity 兼容模式选区".into())))
                .unwrap();

        assert_eq!(result.as_deref(), Some("AntiGravity 兼容模式选区"));
    }

    #[test]
    fn uia_success_does_not_touch_compatibility_reader() {
        let fallback_called = Cell::new(false);

        let result = read_with_compatibility(
            || Ok(Some("来自 UIA".into())),
            || {
                fallback_called.set(true);
                Ok(Some("不应读取".into()))
            },
        )
        .unwrap();

        assert_eq!(result.as_deref(), Some("来自 UIA"));
        assert!(!fallback_called.get());
    }

    #[test]
    fn selection_rect_is_consumed_once_and_rejects_implausible_bounds() {
        clear_selection_rect();
        remember_selection_rect(RECT {
            left: 100,
            top: 200,
            right: 500,
            bottom: 260,
        });
        assert_eq!(
            take_selection_rect(),
            Some(RECT {
                left: 100,
                top: 200,
                right: 500,
                bottom: 260,
            })
        );
        assert_eq!(take_selection_rect(), None);

        remember_selection_rect(RECT {
            left: 0,
            top: 0,
            right: 1600,
            bottom: 900,
        });
        assert_eq!(take_selection_rect(), None);
    }
}
