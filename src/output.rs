//! Output formatting and color handling (design.md D16).
//!
//! Two output modes: `human` (default, colorized when stdout is a TTY) and
//! `json` (stable schema, no color, never includes ANSI codes). Color is
//! disabled by `NO_COLOR=1`, `--no-color`, or non-TTY stdout.

use std::io::{self, IsTerminal};

use anstyle::{AnsiColor, Color, Style};
use clap::ValueEnum;

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum OutputFormat {
    #[default]
    Human,
    Json,
}

/// Should ANSI styling be applied to stdout right now?
///
/// Order of overrides (highest first): `--no-color` flag, `NO_COLOR` env var
/// (any non-empty value), pipe-detection on stdout, otherwise enabled.
pub fn use_color(no_color_flag: bool) -> bool {
    if no_color_flag {
        return false;
    }
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    io::stdout().is_terminal()
}

/// Wrap a string with the given style if `enabled`, otherwise return it raw.
///
/// Using a tiny helper keeps style application uniform across subcommands and
/// guarantees we never emit ANSI sequences when color is disabled.
pub fn styled(enabled: bool, style: Style, s: &str) -> String {
    if !enabled {
        return s.to_string();
    }
    let prefix = style.render();
    let reset = style.render_reset();
    format!("{prefix}{s}{reset}")
}

/// Pre-rolled style for "active" markers in `list` / `current`.
pub fn style_active() -> Style {
    Style::new()
        .fg_color(Some(Color::Ansi(AnsiColor::Green)))
        .bold()
}

/// Pre-rolled style for env names in human output.
pub fn style_env_name() -> Style {
    Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)))
}

/// Pre-rolled style for warnings on stderr.
pub fn style_warn() -> Style {
    Style::new()
        .fg_color(Some(Color::Ansi(AnsiColor::Yellow)))
        .bold()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_color_flag_disables_color() {
        assert!(!use_color(true));
    }

    #[test]
    fn styled_passes_through_when_disabled() {
        let s = styled(false, style_active(), "active");
        assert_eq!(s, "active");
        assert!(!s.contains('\x1b'));
    }
}
