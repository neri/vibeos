//! panic 処理: シリアルにメッセージを出し、画面を赤くして止まる

use core::fmt::{self, Write};
use core::panic::PanicInfo;

use vibe_font::FONT_8X8;
use vibe_gfx::palette;

use crate::arch::cpu;
use crate::drivers::{serial, vga};

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    cpu::disable_interrupts();

    // SAFETY: ここからは通常の処理に戻らないので、ロック中だったものも強制的に使う
    unsafe { serial::panic_write_fmt(format_args!("\nKERNEL PANIC: {}\n", info)) };

    let mut text = FixedBuf::<512>::new();
    let _ = write!(text, "{}", info.message());
    if let Some(loc) = info.location() {
        let _ = write!(text, "\n\nat {}:{}", loc.file(), loc.line());
    }

    // SAFETY: 同上
    let screen = unsafe { vga::SCREEN.force_get() };
    // アプリが前面に出ていたり独自のパレットにしていても、コンソールに戻して見えるようにする
    vga::set_palette(&palette::standard());
    screen.set_foreground(vga::CONSOLE);
    screen.set_cursor_visible(false);
    {
        let font = &FONT_8X8;
        let mut s = screen.surface();
        s.clear(palette::RED);
        s.draw_text(font, 8, 8, "KERNEL PANIC", palette::YELLOW, None);
        let cols = (vga::WIDTH - 16) / font.width;
        let (mut col, mut y) = (0, 24);
        for ch in text.as_str().chars() {
            if ch == '\n' || col >= cols {
                col = 0;
                y += font.height as i32 + 2;
                if ch == '\n' {
                    continue;
                }
            }
            s.draw_char(font, 8 + (col * font.width) as i32, y, ch, palette::WHITE, None);
            col += 1;
        }
    }
    screen.mark_dirty(0, vga::HEIGHT);
    screen.present();

    #[cfg(feature = "qemu-test")]
    crate::drivers::qemu::exit(crate::drivers::qemu::EXIT_FAILURE);

    #[cfg(not(feature = "qemu-test"))]
    cpu::halt_forever()
}

/// ヒープを使わずに文字列を組み立てるための固定長バッファ（あふれた分は捨てる）
struct FixedBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> FixedBuf<N> {
    fn new() -> Self {
        FixedBuf { buf: [0; N], len: 0 }
    }

    fn as_str(&self) -> &str {
        // 文字の途中で切れていても、有効な部分までを返す
        match core::str::from_utf8(&self.buf[..self.len]) {
            Ok(s) => s,
            Err(e) => core::str::from_utf8(&self.buf[..e.valid_up_to()]).unwrap_or(""),
        }
    }
}

impl<const N: usize> Write for FixedBuf<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let n = s.len().min(N - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}
