// Loaded into the SDM process under test, for its GUI thread only, by tests/gui.
//
// iced acts on a click using the cursor position it last heard about, and winit
// asks Windows to report the mouse leaving. With posted mouse messages the real
// cursor is elsewhere, so Windows reports a leave at once and iced drops the
// position. Swallowing WM_MOUSELEAVE lets posted clicks hit the real widgets of the
// unmodified binary, without touching the real mouse.
//
// It also models a drag across monitors. `SdmTestDpiStorm` (wParam: flips, lParam:
// the other monitor's DPI) enters the drag loop's state, flips between the two
// scales, and after each flip applies the size Windows worked out, asking
// WM_GETDPISCALEDSIZE first, as the move loop's own bookkeeping can.
#![allow(non_snake_case)]

#[repr(C)]
#[derive(Clone, Copy)]
struct POINT {
    x: i32,
    y: i32,
}
#[repr(C)]
struct MSG {
    hwnd: isize,
    message: u32,
    wParam: usize,
    lParam: isize,
    time: u32,
    pt: POINT,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RECT {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SIZE {
    cx: i32,
    cy: i32,
}

#[link(name = "user32")]
extern "system" {
    fn CallNextHookEx(h: isize, code: i32, w: usize, l: isize) -> isize;
    fn RegisterWindowMessageW(s: *const u16) -> u32;
    fn SendMessageW(h: isize, m: u32, w: usize, l: isize) -> isize;
    fn GetWindowRect(h: isize, r: *mut RECT) -> i32;
    fn GetDpiForWindow(h: isize) -> u32;
    fn SetWindowPos(h: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, f: u32) -> i32;
}

const WM_NULL: u32 = 0;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_DPICHANGED: u32 = 0x02E0;
const WM_GETDPISCALEDSIZE: u32 = 0x02E4;
const WM_ENTERSIZEMOVE: u32 = 0x0231;
const WM_EXITSIZEMOVE: u32 = 0x0232;
const PM_REMOVE: usize = 1;
const SWP_NOMOVE: u32 = 0x2;
const SWP_NOZORDER: u32 = 0x4;
const SWP_NOACTIVATE: u32 = 0x10;

static mut STORM_MSG: u32 = 0;

unsafe fn storm_msg() -> u32 {
    if STORM_MSG == 0 {
        let name: Vec<u16> = "SdmTestDpiStorm\0".encode_utf16().collect();
        STORM_MSG = RegisterWindowMessageW(name.as_ptr());
    }
    STORM_MSG
}

unsafe fn flip(hwnd: isize, from: u32, to: u32) {
    let mut r = RECT::default();
    GetWindowRect(hwnd, &mut r);
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    // What Windows does: the app's answer if it has one, else scale the whole frame.
    let mut asked = SIZE::default();
    let answered =
        SendMessageW(hwnd, WM_GETDPISCALEDSIZE, to as usize, &mut asked as *mut SIZE as isize) != 0;
    let suggested = if answered {
        asked
    } else {
        SIZE {
            cx: (w as i64 * to as i64 / from as i64) as i32,
            cy: (h as i64 * to as i64 / from as i64) as i32,
        }
    };
    let rect = RECT {
        left: r.left,
        top: r.top,
        right: r.left + suggested.cx,
        bottom: r.top + suggested.cy,
    };
    let dpi = (to as usize) | ((to as usize) << 16);
    SendMessageW(hwnd, WM_DPICHANGED, dpi, &rect as *const RECT as isize);
    // The move loop's own idea of the window's size.
    SetWindowPos(hwnd, 0, 0, 0, suggested.cx, suggested.cy, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
}

#[no_mangle]
pub unsafe extern "system" fn hook(code: i32, w: usize, l: isize) -> isize {
    if code >= 0 && w == PM_REMOVE {
        let msg = &mut *(l as *mut MSG);
        if msg.message == WM_MOUSELEAVE {
            msg.message = WM_NULL;
        } else if msg.message == storm_msg() {
            let (hwnd, flips, other) = (msg.hwnd, msg.wParam, msg.lParam as u32);
            msg.message = WM_NULL;
            let home = GetDpiForWindow(hwnd);
            SendMessageW(hwnd, WM_ENTERSIZEMOVE, 0, 0);
            let mut at = home;
            for _ in 0..flips {
                let to = if at == home { other } else { home };
                flip(hwnd, at, to);
                at = to;
            }
            SendMessageW(hwnd, WM_EXITSIZEMOVE, 0, 0);
        }
    }
    CallNextHookEx(0, code, w, l)
}
