const BOX_INNER: usize = 60;

// --- GOP rendering constants (UEFI target) ---

#[cfg(feature = "uefi-target")]
const FONT_W: usize = 8;
#[cfg(feature = "uefi-target")]
const FONT_H: usize = 16;

#[cfg(feature = "uefi-target")]
use uefi::proto::console::gop::{GraphicsOutput, BltOp, BltPixel, BltRegion};

#[cfg(feature = "uefi-target")]
const BG: BltPixel = BltPixel::new(0x00, 0xC7, 0xFD); // #00C7FD
#[cfg(feature = "uefi-target")]
const FG: BltPixel = BltPixel::new(0xFF, 0xFF, 0xFF); // white

#[cfg(feature = "uefi-target")]
struct GopConsole {
    screen_w: usize,
    screen_h: usize,
    cols: usize,
    rows: usize,
    cur_col: usize,
    cur_row: usize,
}

#[cfg(feature = "uefi-target")]
static mut GOP: Option<GopConsole> = None;

// --- GOP text writer (implements core::fmt::Write) ---

#[cfg(feature = "uefi-target")]
struct GopWriter;

#[cfg(feature = "uefi-target")]
impl core::fmt::Write for GopWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let Ok(handle) = uefi::boot::get_handle_for_protocol::<GraphicsOutput>() else {
            return Err(core::fmt::Error);
        };
        let Ok(mut gop) = uefi::boot::open_protocol_exclusive::<GraphicsOutput>(handle) else {
            return Err(core::fmt::Error);
        };
        let state = unsafe { (&raw mut GOP).as_mut().unwrap().as_mut().ok_or(core::fmt::Error)? };

        for ch in s.chars() {
            if ch == '\n' {
                state.cur_col = 0;
                state.cur_row += 1;
                continue;
            }
            if ch == '\u{0008}' {
                if state.cur_col > 0 {
                    state.cur_col -= 1;
                }
                continue;
            }
            if state.cur_col >= state.cols {
                state.cur_col = 0;
                state.cur_row += 1;
            }
            if state.cur_row >= state.rows {
                break;
            }

            let glyph = crate::font::glyph(ch);
            let mut buf = [BG; 128]; // FONT_W * FONT_H = 8 * 16
            for y in 0..FONT_H {
                for x in 0..FONT_W {
                    if glyph[y] & (0x80 >> x) != 0 {
                        buf[y * FONT_W + x] = FG;
                    }
                }
            }

            let px = state.cur_col * FONT_W;
            let py = state.cur_row * FONT_H;
            let _ = gop.blt(BltOp::BufferToVideo {
                buffer: &buf,
                src: BltRegion::Full,
                dest: (px, py),
                dims: (FONT_W, FONT_H),
            });

            state.cur_col += 1;
        }
        Ok(())
    }
}

// --- Platform-agnostic print helpers ---

macro_rules! ui_print {
    ($($arg:tt)*) => {
        #[cfg(feature = "uefi-target")]
        {
            use core::fmt::Write;
            let mut w = GopWriter;
            let _ = write!(w, $($arg)*);
        }
        #[cfg(feature = "windows-target")]
        { print!($($arg)*); }
    };
}

macro_rules! ui_println {
    () => {
        #[cfg(feature = "uefi-target")]
        {
            use core::fmt::Write;
            let mut w = GopWriter;
            let _ = w.write_str("\n");
        }
        #[cfg(feature = "windows-target")]
        { println!(); }
    };
    ($($arg:tt)*) => {
        #[cfg(feature = "uefi-target")]
        {
            use core::fmt::Write;
            let mut w = GopWriter;
            let _ = write!(w, $($arg)*);
            let _ = w.write_str("\n");
        }
        #[cfg(feature = "windows-target")]
        { println!($($arg)*); }
    };
}

// --- Platform-specific init/clear/input ---

#[cfg(feature = "uefi-target")]
pub fn init() {
    let Ok(handle) = uefi::boot::get_handle_for_protocol::<GraphicsOutput>() else { return };
    let Ok(mut gop) = uefi::boot::open_protocol_exclusive::<GraphicsOutput>(handle) else { return };

    let info = gop.current_mode_info();
    let (w, h) = info.resolution();
    let cols = w / FONT_W;
    let rows = h / FONT_H;

    let _ = gop.blt(BltOp::VideoFill {
        color: BG,
        dest: (0, 0),
        dims: (w, h),
    });

    unsafe {
        GOP = Some(GopConsole {
            screen_w: w,
            screen_h: h,
            cols,
            rows,
            cur_col: 0,
            cur_row: 0,
        });
    }
}

#[cfg(feature = "windows-target")]
pub fn init() {
    // On Windows, just clear screen with ANSI codes
    print!("\x1B[2J\x1B[H");
}

#[cfg(feature = "uefi-target")]
pub fn clear() {
    if let Ok(handle) = uefi::boot::get_handle_for_protocol::<GraphicsOutput>() {
        if let Ok(mut gop) = uefi::boot::open_protocol_exclusive::<GraphicsOutput>(handle) {
            if let Some(state) = unsafe { (&raw mut GOP).as_mut().unwrap() } {
                let _ = gop.blt(BltOp::VideoFill {
                    color: BG,
                    dest: (0, 0),
                    dims: (state.screen_w, state.screen_h),
                });
                state.cur_col = 0;
                state.cur_row = 0;
            }
        }
    }
}

#[cfg(feature = "windows-target")]
pub fn clear() {
    print!("\x1B[2J\x1B[H");
}

/// Output blank lines to vertically center `content_lines` rows of content.
pub fn v_center(content_lines: usize) {
    #[cfg(feature = "uefi-target")]
    {
        let rows = unsafe {
            (&raw const GOP).as_ref().unwrap().as_ref().map(|s| s.rows).unwrap_or(25)
        };
        let skip = if rows > content_lines { (rows - content_lines) / 2 } else { 0 };
        for _ in 0..skip {
            ui_println!();
        }
    }
    #[cfg(feature = "windows-target")]
    {
        let _ = content_lines;
        ui_println!();
    }
}

#[cfg(feature = "uefi-target")]
pub fn show_cursor() {
    // No hardware cursor in GOP mode
}

#[cfg(feature = "windows-target")]
pub fn show_cursor() {}

#[cfg(feature = "uefi-target")]
fn margin() -> usize {
    let cols = unsafe { (&raw const GOP).as_ref().unwrap().as_ref().map(|s| s.cols).unwrap_or(80) };
    let box_total = BOX_INNER + 2;
    if cols > box_total { (cols - box_total) / 2 } else { 0 }
}

#[cfg(feature = "windows-target")]
fn margin() -> usize {
    let cols = 80usize; // assume 80 columns on terminal
    let box_total = BOX_INNER + 2;
    if cols > box_total { (cols - box_total) / 2 } else { 0 }
}

#[cfg(feature = "uefi-target")]
pub fn wait_key() -> char {
    uefi::system::with_stdin(|input| {
        if let Some(event) = input.wait_for_key_event() {
            let mut events = [event];
            if uefi::boot::wait_for_event(&mut events).is_ok() {
                if let Ok(Some(key)) = input.read_key() {
                    match key {
                        uefi::proto::console::text::Key::Printable(c) => {
                            return char::from(c);
                        }
                        _ => {}
                    }
                }
            }
        }
        '\0'
    })
}

#[cfg(feature = "windows-target")]
pub fn wait_key() -> char {
    use windows_sys::Win32::Storage::FileSystem::*;

    unsafe {
        let stdin = windows_sys::Win32::System::Console::GetStdHandle(
            windows_sys::Win32::System::Console::STD_INPUT_HANDLE,
        );

        let mut old_mode: u32 = 0;
        windows_sys::Win32::System::Console::GetConsoleMode(stdin, &mut old_mode);
        windows_sys::Win32::System::Console::SetConsoleMode(stdin, 0);

        let mut buf = [0u8; 1];
        let mut read: u32 = 0;
        ReadFile(stdin, buf.as_mut_ptr(), 1, &mut read, std::ptr::null_mut());

        windows_sys::Win32::System::Console::SetConsoleMode(stdin, old_mode);

        buf[0] as char
    }
}

// --- Box drawing (platform-agnostic, uses ui_print!/ui_println!) ---

fn print_spaces(n: usize) {
    for _ in 0..n {
        ui_print!(" ");
    }
}

pub fn box_top() {
    let m = margin();
    print_spaces(m);
    ui_print!("\u{2554}");
    for _ in 0..BOX_INNER {
        ui_print!("\u{2550}");
    }
    ui_println!("\u{2557}");
}

pub fn box_bottom() {
    let m = margin();
    print_spaces(m);
    ui_print!("\u{255A}");
    for _ in 0..BOX_INNER {
        ui_print!("\u{2550}");
    }
    ui_println!("\u{255D}");
}

pub fn box_sep() {
    let m = margin();
    print_spaces(m);
    ui_print!("\u{2560}");
    for _ in 0..BOX_INNER {
        ui_print!("\u{2550}");
    }
    ui_println!("\u{2563}");
}

pub fn box_blank() {
    let m = margin();
    print_spaces(m);
    ui_print!("\u{2551}");
    print_spaces(BOX_INNER);
    ui_println!("\u{2551}");
}

pub fn box_line(text: &str) {
    let m = margin();
    let content_w = BOX_INNER - 2;
    let text_len = text.chars().count().min(content_w);

    print_spaces(m);
    ui_print!("\u{2551} ");
    let mut printed = 0;
    for ch in text.chars() {
        if printed >= content_w { break; }
        ui_print!("{}", ch);
        printed += 1;
    }
    print_spaces(content_w - text_len);
    ui_println!(" \u{2551}");
}

pub fn box_center(text: &str) {
    let m = margin();
    let text_len = text.chars().count().min(BOX_INNER);
    let left_pad = (BOX_INNER - text_len) / 2;
    let right_pad = BOX_INNER - text_len - left_pad;

    print_spaces(m);
    ui_print!("\u{2551}");
    print_spaces(left_pad);
    let mut printed = 0;
    for ch in text.chars() {
        if printed >= BOX_INNER { break; }
        ui_print!("{}", ch);
        printed += 1;
    }
    print_spaces(right_pad);
    ui_println!("\u{2551}");
}

pub fn box_kv(label: &str, value: &str) {
    let m = margin();
    let label_w: usize = 20;
    let content_w = BOX_INNER - 2;

    print_spaces(m);
    ui_print!("\u{2551} ");

    let mut printed = 0;
    for ch in label.chars() {
        if printed >= label_w { break; }
        ui_print!("{}", ch);
        printed += 1;
    }
    print_spaces(label_w - printed);

    let value_w = content_w - label_w;
    let mut printed = 0;
    for ch in value.chars() {
        if printed >= value_w { break; }
        ui_print!("{}", ch);
        printed += 1;
    }
    print_spaces(value_w - printed);

    ui_println!(" \u{2551}");
}

/// Read a line of printable ASCII from the keyboard into `buf`, echoing as typed.
/// Terminates on Enter (CR/LF). Backspace erases the last char.
/// Returns the number of bytes stored in `buf`.
pub fn read_line(buf: &mut [u8]) -> usize {
    let mut len = 0usize;
    loop {
        let ch = wait_key();
        let code = ch as u32;
        if code == 0x0D || code == 0x0A {
            ui_println!();
            return len;
        }
        if code == 0x08 || code == 0x7F {
            if len > 0 {
                len -= 1;
                ui_print!("{}", "\u{0008} \u{0008}");
            }
            continue;
        }
        if code < 0x20 || code > 0x7E {
            continue;
        }
        if len >= buf.len() {
            continue;
        }
        buf[len] = ch as u8;
        len += 1;
        ui_print!("{}", ch);
    }
}

pub fn show_working(message: &str) {
    clear();
    v_center(5);
    box_top();
    box_blank();
    box_center(message);
    box_blank();
    box_bottom();
}

pub fn press_any_key() {
    box_sep();
    box_line("Press any key to continue...");
    box_bottom();
    wait_key();
}
