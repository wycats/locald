use console::Emoji;

pub static CHECK: Emoji<'_, '_> = Emoji("✅", "v");
pub static CROSS: Emoji<'_, '_> = Emoji("❌", "x");
pub static PACKAGE: Emoji<'_, '_> = Emoji("📦", "[]");
pub static WARN: Emoji<'_, '_> = Emoji("⚠", "!");
#[cfg(target_os = "macos")]
pub static INFO: Emoji<'_, '_> = Emoji("ℹ️", "i");
pub static ROCKET: Emoji<'_, '_> = Emoji("🚀", ">");
pub static DOT: Emoji<'_, '_> = Emoji("•", "-");
