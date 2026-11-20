//! Windows 原生标题栏配色：通过 DWM API（DwmSetWindowAttribute）把标题栏
//! 背景/边框/文字颜色设置为与页面主题一致，消除原生标题栏与应用内容区的色差。
//! 非 Windows 平台此模块为空实现。

#[cfg(target_os = "windows")]
pub fn apply(window: &gpui_kit::Window, cx: &gpui_kit::App) {
    use gpui_kit::component::Theme;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    // GPUI 自身也有 window_handle() 固有方法（返回 AnyWindowHandle），
    // 这里显式走 raw-window-handle 的 trait 拿原生句柄
    let Ok(wh) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = wh.as_raw() else {
        return;
    };

    let theme = Theme::global(cx);
    let bg: gpui_kit::Rgba = theme.background.into();
    let fg: gpui_kit::Rgba = theme.foreground.into();

    // COLORREF 布局为 0x00BBGGRR；Rgba 分量为 0.0..1.0 浮点
    let to_ref = |c: gpui_kit::Rgba| -> u32 {
        let chan = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
        chan(c.r) | (chan(c.g) << 8) | (chan(c.b) << 16)
    };
    let bg_ref = to_ref(bg);
    let fg_ref = to_ref(fg);

    unsafe {
        use windows_sys::Win32::Graphics::Dwm::{
            DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR,
        };
        let hwnd = handle.hwnd.get() as *mut core::ffi::c_void;
        let set = |attr: u32, value: u32| {
            DwmSetWindowAttribute(
                hwnd,
                attr,
                &value as *const u32 as *const core::ffi::c_void,
                core::mem::size_of::<u32>() as u32,
            );
        };
        // 背景与边框 = 页面背景色；文字 = 页面前景色
        set(DWMWA_CAPTION_COLOR as u32, bg_ref);
        set(DWMWA_BORDER_COLOR as u32, bg_ref);
        set(DWMWA_TEXT_COLOR as u32, fg_ref);
    }
}

#[cfg(not(target_os = "windows"))]
pub fn apply(_window: &gpui_kit::Window, _cx: &gpui_kit::App) {}
