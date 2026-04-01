use console::Emoji;
use std::io::IsTerminal;
use std::sync::OnceLock;

pub static CHECK: Emoji<'_, '_> = Emoji("✅", "v");
pub static CROSS: Emoji<'_, '_> = Emoji("❌", "x");
pub static PACKAGE: Emoji<'_, '_> = Emoji("📦", "[]");
pub static WARN: Emoji<'_, '_> = Emoji("⚠", "!");
pub static INFO: Emoji<'_, '_> = Emoji("ℹ️", "i");
#[allow(dead_code)]
pub static ROCKET: Emoji<'_, '_> = Emoji("🚀", ">");
#[allow(dead_code)]
pub static DOT: Emoji<'_, '_> = Emoji("•", "-");

static COLORS_ENABLED: OnceLock<bool> = OnceLock::new();

pub fn colors_enabled() -> bool {
    *COLORS_ENABLED.get_or_init(|| {
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        std::io::stdout().is_terminal()
    })
}

pub fn configure_colors() -> bool {
    let enabled = colors_enabled();
    console::set_colors_enabled(enabled);
    if !enabled {
        crossterm::style::force_color_output(false);
    }
    enabled
}
