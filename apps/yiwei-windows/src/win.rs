//! 用到的 Windows 系统接口（自己声明，不引第三方库）
#![allow(non_snake_case, non_camel_case_types, clippy::upper_case_acronyms, dead_code)]
use std::ffi::c_void;

pub type HWND = isize;
pub type HANDLE = isize;
pub type HMENU = isize;
pub type HICON = isize;
pub type HHOOK = isize;
pub type HKEY = isize;
pub type WPARAM = usize;
pub type LPARAM = isize;
pub type LRESULT = isize;
pub type BOOL = i32;
pub type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;
pub type HookProc = unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT;

#[repr(C)] #[derive(Clone, Copy, Default)] pub struct POINT { pub x: i32, pub y: i32 }
#[repr(C)] pub struct MSG { pub hwnd: HWND, pub message: u32, pub wParam: WPARAM, pub lParam: LPARAM, pub time: u32, pub pt: POINT, pub lPrivate: u32 }
#[repr(C)] pub struct WNDCLASSEXW {
    pub cbSize: u32, pub style: u32, pub lpfnWndProc: Option<WndProc>, pub cbClsExtra: i32, pub cbWndExtra: i32,
    pub hInstance: HANDLE, pub hIcon: HICON, pub hCursor: HANDLE, pub hbrBackground: HANDLE,
    pub lpszMenuName: *const u16, pub lpszClassName: *const u16, pub hIconSm: HICON,
}
#[repr(C)] pub struct NOTIFYICONDATAW {
    pub cbSize: u32, pub hWnd: HWND, pub uID: u32, pub uFlags: u32, pub uCallbackMessage: u32, pub hIcon: HICON,
    pub szTip: [u16; 128], pub dwState: u32, pub dwStateMask: u32, pub szInfo: [u16; 256], pub uVersion: u32,
    pub szInfoTitle: [u16; 64], pub dwInfoFlags: u32, pub guidItem: [u8; 16], pub hBalloonIcon: HICON,
}
#[repr(C)] pub struct MSLLHOOKSTRUCT { pub pt: POINT, pub mouseData: u32, pub flags: u32, pub time: u32, pub dwExtraInfo: usize }
#[repr(C)] pub struct KBDLLHOOKSTRUCT { pub vkCode: u32, pub scanCode: u32, pub flags: u32, pub time: u32, pub dwExtraInfo: usize }
#[repr(C)] #[derive(Clone, Copy)] pub struct MOUSEINPUT { pub dx: i32, pub dy: i32, pub mouseData: u32, pub dwFlags: u32, pub time: u32, pub dwExtraInfo: usize }
#[repr(C)] #[derive(Clone, Copy)] pub struct KEYBDINPUT { pub wVk: u16, pub wScan: u16, pub dwFlags: u32, pub time: u32, pub dwExtraInfo: usize }
#[repr(C)] #[derive(Clone, Copy)] pub union INPUT_U { pub mi: MOUSEINPUT, pub ki: KEYBDINPUT }
#[repr(C)] #[derive(Clone, Copy)] pub struct INPUT { pub kind: u32, pub u: INPUT_U }

pub const WM_DESTROY: u32 = 0x0002;
pub const WM_COMMAND: u32 = 0x0111;
pub const WM_TIMER: u32 = 0x0113;
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_KEYUP: u32 = 0x0101;
pub const WM_SYSKEYDOWN: u32 = 0x0104;
pub const WM_SYSKEYUP: u32 = 0x0105;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_RBUTTONDOWN: u32 = 0x0204;
pub const WM_RBUTTONUP: u32 = 0x0205;
pub const WM_MBUTTONDOWN: u32 = 0x0207;
pub const WM_MBUTTONUP: u32 = 0x0208;
pub const WM_MOUSEWHEEL: u32 = 0x020A;
pub const WM_XBUTTONDOWN: u32 = 0x020B;
pub const WM_XBUTTONUP: u32 = 0x020C;
pub const WM_MOUSEHWHEEL: u32 = 0x020E;
pub const WM_DISPLAYCHANGE: u32 = 0x007E;
pub const WM_APP: u32 = 0x8000;
pub const WM_QUIT: u32 = 0x0012;
pub const WH_KEYBOARD_LL: i32 = 13;
pub const WH_MOUSE_LL: i32 = 14;
pub const LLKHF_EXTENDED: u32 = 0x01;
pub const LLKHF_INJECTED: u32 = 0x10;
pub const LLMHF_INJECTED: u32 = 0x01;

pub const NIM_ADD: u32 = 0; pub const NIM_MODIFY: u32 = 1; pub const NIM_DELETE: u32 = 2;
pub const NIF_MESSAGE: u32 = 1; pub const NIF_ICON: u32 = 2; pub const NIF_TIP: u32 = 4; pub const NIF_INFO: u32 = 0x10;
pub const NIIF_INFO: u32 = 1;

pub const MF_STRING: u32 = 0; pub const MF_GRAYED: u32 = 1; pub const MF_CHECKED: u32 = 8; pub const MF_SEPARATOR: u32 = 0x800;
pub const TPM_RIGHTBUTTON: u32 = 2; pub const TPM_BOTTOMALIGN: u32 = 0x20; pub const TPM_RETURNCMD: u32 = 0x100;

pub const INPUT_MOUSE: u32 = 0; pub const INPUT_KEYBOARD: u32 = 1;
pub const MOUSEEVENTF_MOVE: u32 = 1; pub const MOUSEEVENTF_LEFTDOWN: u32 = 2; pub const MOUSEEVENTF_LEFTUP: u32 = 4;
pub const MOUSEEVENTF_RIGHTDOWN: u32 = 8; pub const MOUSEEVENTF_RIGHTUP: u32 = 0x10; pub const MOUSEEVENTF_MIDDLEDOWN: u32 = 0x20;
pub const MOUSEEVENTF_MIDDLEUP: u32 = 0x40; pub const MOUSEEVENTF_XDOWN: u32 = 0x80; pub const MOUSEEVENTF_XUP: u32 = 0x100;
pub const MOUSEEVENTF_WHEEL: u32 = 0x800; pub const MOUSEEVENTF_HWHEEL: u32 = 0x1000; pub const MOUSEEVENTF_VIRTUALDESK: u32 = 0x4000;
pub const MOUSEEVENTF_ABSOLUTE: u32 = 0x8000;
pub const KEYEVENTF_EXTENDEDKEY: u32 = 1; pub const KEYEVENTF_KEYUP: u32 = 2; pub const KEYEVENTF_SCANCODE: u32 = 8;

pub const SM_XVIRTUALSCREEN: i32 = 76; pub const SM_YVIRTUALSCREEN: i32 = 77; pub const SM_CXVIRTUALSCREEN: i32 = 78; pub const SM_CYVIRTUALSCREEN: i32 = 79;
pub const SM_CXSCREEN: i32 = 0; pub const SM_CYSCREEN: i32 = 1;
pub const MB_YESNO: u32 = 4; pub const MB_OK: u32 = 0; pub const MB_ICONQUESTION: u32 = 0x20; pub const MB_ICONWARNING: u32 = 0x30;
pub const MB_SETFOREGROUND: u32 = 0x10000; pub const MB_TOPMOST: u32 = 0x40000; pub const IDYES: i32 = 6;
pub const CF_DIB: u32 = 8; pub const CF_DIBV5: u32 = 17;
pub const CF_UNICODETEXT: u32 = 13; pub const GMEM_MOVEABLE: u32 = 2;
pub const SPI_SETCURSORS: u32 = 0x57;
pub const HKEY_CURRENT_USER: HKEY = 0x8000_0001u32 as i32 as isize;
pub const REG_SZ: u32 = 1; pub const RRF_RT_REG_SZ: u32 = 2;
pub const ERROR_ALREADY_EXISTS: u32 = 183;

#[link(name = "user32")]
extern "system" {
    pub fn RegisterClassExW(c: *const WNDCLASSEXW) -> u16;
    pub fn CreateWindowExW(ex: u32, class: *const u16, name: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: HWND, menu: HMENU, inst: HANDLE, p: *const c_void) -> HWND;
    pub fn DefWindowProcW(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT;
    pub fn GetMessageW(m: *mut MSG, h: HWND, a: u32, b: u32) -> BOOL;
    pub fn TranslateMessage(m: *const MSG) -> BOOL;
    pub fn DispatchMessageW(m: *const MSG) -> LRESULT;
    pub fn PostMessageW(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> BOOL;
    pub fn PostThreadMessageW(t: u32, m: u32, w: WPARAM, l: LPARAM) -> BOOL;
    pub fn PostQuitMessage(code: i32);
    pub fn SetWindowsHookExW(id: i32, f: HookProc, inst: HANDLE, thread: u32) -> HHOOK;
    pub fn UnhookWindowsHookEx(h: HHOOK) -> BOOL;
    pub fn CallNextHookEx(h: HHOOK, code: i32, w: WPARAM, l: LPARAM) -> LRESULT;
    pub fn SendInput(n: u32, inputs: *const INPUT, size: i32) -> u32;
    pub fn SetCursorPos(x: i32, y: i32) -> BOOL;
    pub fn GetCursorPos(p: *mut POINT) -> BOOL;
    pub fn GetAsyncKeyState(vk: i32) -> i16;
    pub fn GetSystemMetrics(i: i32) -> i32;
    pub fn CreatePopupMenu() -> HMENU;
    pub fn AppendMenuW(m: HMENU, flags: u32, id: usize, text: *const u16) -> BOOL;
    pub fn TrackPopupMenu(m: HMENU, flags: u32, x: i32, y: i32, r: i32, h: HWND, rect: *const c_void) -> BOOL;
    pub fn DestroyMenu(m: HMENU) -> BOOL;
    pub fn SetForegroundWindow(h: HWND) -> BOOL;
    pub fn MessageBoxW(h: HWND, text: *const u16, caption: *const u16, t: u32) -> i32;
    pub fn OpenClipboard(h: HWND) -> BOOL;
    pub fn CloseClipboard() -> BOOL;
    pub fn EmptyClipboard() -> BOOL;
    pub fn GetClipboardData(f: u32) -> HANDLE;
    pub fn SetClipboardData(f: u32, h: HANDLE) -> HANDLE;
    pub fn GetClipboardSequenceNumber() -> u32;
    pub fn EnumClipboardFormats(f: u32) -> u32;
    pub fn IsClipboardFormatAvailable(f: u32) -> BOOL;
    pub fn RegisterClipboardFormatW(s: *const u16) -> u32;
    pub fn CreateIconFromResourceEx(bits: *const u8, size: u32, icon: BOOL, ver: u32, cx: i32, cy: i32, flags: u32) -> HICON;
    pub fn CreateCursor(inst: HANDLE, x: i32, y: i32, w: i32, h: i32, and: *const u8, xor: *const u8) -> HANDLE;
    pub fn SetSystemCursor(c: HANDLE, id: u32) -> BOOL;
    pub fn SystemParametersInfoW(a: u32, b: u32, p: *mut c_void, f: u32) -> BOOL;
    pub fn RegisterWindowMessageW(s: *const u16) -> u32;
    pub fn SetTimer(h: HWND, id: usize, ms: u32, f: *const c_void) -> usize;
    pub fn KillTimer(h: HWND, id: usize) -> BOOL;
    pub fn GetKeyState(vk: i32) -> i16;
}
#[link(name = "kernel32")]
extern "system" {
    pub fn GetModuleHandleW(n: *const u16) -> HANDLE;
    pub fn CreateMutexW(a: *const c_void, own: BOOL, name: *const u16) -> HANDLE;
    pub fn GetLastError() -> u32;
    pub fn CloseHandle(h: HANDLE) -> BOOL;
    pub fn GlobalAlloc(f: u32, n: usize) -> HANDLE;
    pub fn GlobalLock(h: HANDLE) -> *mut c_void;
    pub fn GlobalUnlock(h: HANDLE) -> BOOL;
    pub fn GetCurrentThreadId() -> u32;
    pub fn GlobalSize(h: HANDLE) -> usize;
    pub fn GlobalFree(h: HANDLE) -> HANDLE;
}
#[link(name = "wtsapi32")]
extern "system" {
    pub fn WTSRegisterSessionNotification(h: HWND, flags: u32) -> BOOL;
}
#[link(name = "shell32")]
extern "system" {
    pub fn Shell_NotifyIconW(m: u32, d: *const NOTIFYICONDATAW) -> BOOL;
}
#[link(name = "advapi32")]
extern "system" {
    pub fn RegSetKeyValueW(k: HKEY, sub: *const u16, name: *const u16, t: u32, data: *const c_void, n: u32) -> i32;
    pub fn RegDeleteKeyValueW(k: HKEY, sub: *const u16, name: *const u16) -> i32;
    pub fn RegGetValueW(k: HKEY, sub: *const u16, name: *const u16, flags: u32, t: *mut u32, data: *mut c_void, n: *mut u32) -> i32;
}

/// Rust 字符串 → 以 0 结尾的 UTF-16
pub fn w(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }
/// 写进定长的 UTF-16 数组（托盘提示之类），太长就截断
pub fn fill(dst: &mut [u16], s: &str) {
    let mut n = 0;
    for c in s.encode_utf16() { if n + 1 >= dst.len() { break } dst[n] = c; n += 1 }
    dst[n] = 0;
}

// ---- 面板要用的：窗口、绘制（GDI+）、显示器 ----
#[repr(C)] #[derive(Clone, Copy, Default)] pub struct RECT { pub left: i32, pub top: i32, pub right: i32, pub bottom: i32 }
#[repr(C)] pub struct PAINTSTRUCT { pub hdc: HANDLE, pub fErase: BOOL, pub rcPaint: RECT, pub fRestore: BOOL, pub fIncUpdate: BOOL, pub rgbReserved: [u8; 32] }
#[repr(C)] pub struct MONITORINFO { pub cbSize: u32, pub rcMonitor: RECT, pub rcWork: RECT, pub dwFlags: u32 }
#[repr(C)] pub struct TRACKMOUSEEVENT { pub cbSize: u32, pub dwFlags: u32, pub hwndTrack: HWND, pub dwHoverTime: u32 }
#[repr(C)] #[derive(Clone, Copy, Default, Debug)] pub struct RectF { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }
#[repr(C)] pub struct GdiplusStartupInput { pub version: u32, pub callback: usize, pub no_thread: BOOL, pub no_codecs: BOOL }

pub const WS_POPUP: u32 = 0x8000_0000; pub const WS_CHILD: u32 = 0x4000_0000; pub const WS_VISIBLE: u32 = 0x1000_0000; pub const WS_BORDER: u32 = 0x0080_0000;
pub const ES_AUTOHSCROLL: u32 = 0x80;
pub const WS_EX_TOOLWINDOW: u32 = 0x80; pub const WS_EX_TOPMOST: u32 = 0x8;
pub const CS_DROPSHADOW: u32 = 0x0002_0000;
pub const WM_PAINT: u32 = 0x000F; pub const WM_ACTIVATE: u32 = 0x0006; pub const WM_SETCURSOR: u32 = 0x0020; pub const WM_SETFONT: u32 = 0x0030;
pub const WM_MOUSELEAVE: u32 = 0x02A3; pub const WM_DPICHANGED: u32 = 0x02E0; pub const WM_DROPFILES: u32 = 0x0233; pub const WM_ERASEBKGND: u32 = 0x0014;
pub const SW_HIDE: i32 = 0; pub const SW_SHOWNA: i32 = 8; pub const SW_SHOW: i32 = 5;
pub const SWP_NOZORDER: u32 = 0x4; pub const SWP_NOACTIVATE: u32 = 0x10;
pub const TME_LEAVE: u32 = 2; pub const MONITOR_DEFAULTTONEAREST: u32 = 2;
pub const SRCCOPY: u32 = 0x00CC_0020;
pub const RRF_RT_REG_DWORD: u32 = 0x10;

#[link(name = "user32")]
extern "system" {
    pub fn BeginPaint(h: HWND, p: *mut PAINTSTRUCT) -> HANDLE;
    pub fn EndPaint(h: HWND, p: *const PAINTSTRUCT) -> BOOL;
    pub fn InvalidateRect(h: HWND, r: *const RECT, erase: BOOL) -> BOOL;
    pub fn ShowWindow(h: HWND, cmd: i32) -> BOOL;
    pub fn IsWindowVisible(h: HWND) -> BOOL;
    pub fn SetWindowPos(h: HWND, after: HWND, x: i32, y: i32, cx: i32, cy: i32, f: u32) -> BOOL;
    pub fn DestroyWindow(h: HWND) -> BOOL;
    pub fn GetDC(h: HWND) -> HANDLE;
    pub fn ReleaseDC(h: HWND, dc: HANDLE) -> i32;
    pub fn GetDpiForWindow(h: HWND) -> u32;
    pub fn GetDpiForSystem() -> u32;
    pub fn SetCapture(h: HWND) -> HWND;
    pub fn ReleaseCapture() -> BOOL;
    pub fn TrackMouseEvent(t: *mut TRACKMOUSEEVENT) -> BOOL;
    pub fn MonitorFromPoint(p: POINT, f: u32) -> HANDLE;
    pub fn GetMonitorInfoW(m: HANDLE, i: *mut MONITORINFO) -> BOOL;
    pub fn SendMessageW(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT;
    pub fn GetWindowTextW(h: HWND, s: *mut u16, n: i32) -> i32;
    pub fn SetFocus(h: HWND) -> HWND;
    pub fn LoadCursorW(inst: HANDLE, id: usize) -> HANDLE;
    pub fn SetCursor(c: HANDLE) -> HANDLE;
    pub fn GetTickCount() -> u32;
    pub fn GetWindowRect(h: HWND, r: *mut RECT) -> BOOL;
}
#[link(name = "gdi32")]
extern "system" {
    pub fn CreateCompatibleDC(dc: HANDLE) -> HANDLE;
    pub fn CreateCompatibleBitmap(dc: HANDLE, w: i32, h: i32) -> HANDLE;
    pub fn SelectObject(dc: HANDLE, o: HANDLE) -> HANDLE;
    pub fn BitBlt(d: HANDLE, x: i32, y: i32, w: i32, h: i32, s: HANDLE, sx: i32, sy: i32, rop: u32) -> BOOL;
    pub fn DeleteObject(o: HANDLE) -> BOOL;
    pub fn DeleteDC(dc: HANDLE) -> BOOL;
    pub fn CreateFontW(h: i32, w: i32, esc: i32, ori: i32, weight: i32, it: u32, ul: u32, so: u32, cs: u32, op: u32, cp: u32, q: u32, pf: u32, face: *const u16) -> HANDLE;
}
#[link(name = "dwmapi")]
extern "system" {
    pub fn DwmSetWindowAttribute(h: HWND, attr: u32, v: *const c_void, n: u32) -> i32;
}
#[link(name = "gdiplus")]
extern "system" {
    pub fn GdiplusStartup(token: *mut usize, input: *const GdiplusStartupInput, out: *mut c_void) -> i32;
    pub fn GdipCreateFromHDC(dc: HANDLE, g: *mut *mut c_void) -> i32;
    pub fn GdipDeleteGraphics(g: *mut c_void) -> i32;
    pub fn GdipSetSmoothingMode(g: *mut c_void, m: i32) -> i32;
    pub fn GdipSetTextRenderingHint(g: *mut c_void, m: i32) -> i32;
    pub fn GdipScaleWorldTransform(g: *mut c_void, sx: f32, sy: f32, order: i32) -> i32;
    pub fn GdipGraphicsClear(g: *mut c_void, argb: u32) -> i32;
    pub fn GdipCreateSolidFill(argb: u32, b: *mut *mut c_void) -> i32;
    pub fn GdipDeleteBrush(b: *mut c_void) -> i32;
    pub fn GdipCreatePen1(argb: u32, w: f32, unit: i32, p: *mut *mut c_void) -> i32;
    pub fn GdipDeletePen(p: *mut c_void) -> i32;
    pub fn GdipSetPenDashStyle(p: *mut c_void, s: i32) -> i32;
    pub fn GdipSetPenLineCap197819(p: *mut c_void, start: i32, end: i32, dash: i32) -> i32;
    pub fn GdipSetPenLineJoin(p: *mut c_void, j: i32) -> i32;
    pub fn GdipCreatePath(mode: i32, p: *mut *mut c_void) -> i32;
    pub fn GdipDeletePath(p: *mut c_void) -> i32;
    pub fn GdipAddPathArc(p: *mut c_void, x: f32, y: f32, w: f32, h: f32, start: f32, sweep: f32) -> i32;
    pub fn GdipAddPathLine(p: *mut c_void, x1: f32, y1: f32, x2: f32, y2: f32) -> i32;
    pub fn GdipAddPathBezier(p: *mut c_void, x1: f32, y1: f32, x2: f32, y2: f32, x3: f32, y3: f32, x4: f32, y4: f32) -> i32;
    pub fn GdipStartPathFigure(p: *mut c_void) -> i32;
    pub fn GdipClosePathFigure(p: *mut c_void) -> i32;
    pub fn GdipFillPath(g: *mut c_void, b: *mut c_void, p: *mut c_void) -> i32;
    pub fn GdipDrawPath(g: *mut c_void, pen: *mut c_void, p: *mut c_void) -> i32;
    pub fn GdipFillEllipse(g: *mut c_void, b: *mut c_void, x: f32, y: f32, w: f32, h: f32) -> i32;
    pub fn GdipFillRectangle(g: *mut c_void, b: *mut c_void, x: f32, y: f32, w: f32, h: f32) -> i32;
    pub fn GdipCreateFontFamilyFromName(name: *const u16, coll: *mut c_void, f: *mut *mut c_void) -> i32;
    pub fn GdipGetGenericFontFamilySansSerif(f: *mut *mut c_void) -> i32;
    pub fn GdipDeleteFontFamily(f: *mut c_void) -> i32;
    pub fn GdipCreateFont(f: *mut c_void, size: f32, style: i32, unit: i32, font: *mut *mut c_void) -> i32;
    pub fn GdipDeleteFont(f: *mut c_void) -> i32;
    pub fn GdipCreateStringFormat(flags: i32, lang: u16, f: *mut *mut c_void) -> i32;
    pub fn GdipDeleteStringFormat(f: *mut c_void) -> i32;
    pub fn GdipSetStringFormatAlign(f: *mut c_void, a: i32) -> i32;
    pub fn GdipSetStringFormatLineAlign(f: *mut c_void, a: i32) -> i32;
    pub fn GdipSetStringFormatTrimming(f: *mut c_void, t: i32) -> i32;
    pub fn GdipDrawString(g: *mut c_void, s: *const u16, n: i32, font: *mut c_void, r: *const RectF, f: *mut c_void, b: *mut c_void) -> i32;
    pub fn GdipMeasureString(g: *mut c_void, s: *const u16, n: i32, font: *mut c_void, r: *const RectF, f: *mut c_void, out: *mut RectF, chars: *mut i32, lines: *mut i32) -> i32;
}

// ---- 打开资源管理器、气泡被点 ----
pub const NIN_BALLOONUSERCLICK: u32 = 0x405;
#[link(name = "shell32")]
extern "system" {
    pub fn ShellExecuteW(h: HWND, op: *const u16, file: *const u16, params: *const u16, dir: *const u16, show: i32) -> HANDLE;
}
#[link(name = "ole32")]
extern "system" { pub fn CoInitializeEx(r: *const c_void, f: u32) -> i32; pub fn CoTaskMemFree(p: *mut c_void); }

// ---- 新式的选择窗口（和资源管理器一样的那种） ----
#[repr(C)] pub struct GUID(pub u32, pub u16, pub u16, pub [u8; 8]);
pub const CLSID_FILE_OPEN_DIALOG: GUID = GUID(0xDC1C5A9C, 0xE88A, 0x4DDE, [0xA5, 0xA1, 0x60, 0xF8, 0x2A, 0x20, 0xAE, 0xF7]);
pub const IID_IFILE_OPEN_DIALOG: GUID = GUID(0xD57C7288, 0xD4AD, 0x4768, [0xBE, 0x02, 0x9D, 0x96, 0x95, 0x32, 0xD9, 0x60]);
pub const IID_ISHELL_ITEM: GUID = GUID(0x43826D1E, 0xE718, 0x42EE, [0xBC, 0x55, 0xA1, 0xE2, 0x61, 0xC3, 0x7B, 0xFE]);
pub const FOS_PICKFOLDERS: u32 = 0x20; pub const FOS_FORCEFILESYSTEM: u32 = 0x40; pub const FOS_ALLOWMULTISELECT: u32 = 0x200; pub const FOS_PATHMUSTEXIST: u32 = 0x800;
pub const SIGDN_FILESYSPATH: u32 = 0x8005_8000;
pub const CF_HDROP: u32 = 15;
#[link(name = "ole32")]
extern "system" { pub fn CoCreateInstance(clsid: *const GUID, outer: *mut c_void, ctx: u32, iid: *const GUID, out: *mut *mut c_void) -> i32; }
#[link(name = "shell32")]
extern "system" {
    pub fn SHCreateItemFromParsingName(path: *const u16, bc: *mut c_void, iid: *const GUID, out: *mut *mut c_void) -> i32;
    pub fn DragQueryFileW(h: HANDLE, i: u32, buf: *mut u16, n: u32) -> u32;
    pub fn DragAcceptFiles(h: HWND, accept: BOOL);
    pub fn DragFinish(h: HANDLE);
}
