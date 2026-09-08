//! GitHub-ready diagnostics bundle formatter.
//!
//! Produces a collapsible `<details>` markdown block (a bold field header + a
//! log-language fenced block of the last N lines) for pasting into a GitHub issue.
//! Lines are redacted defensively even though the ring already holds redacted text.

use crate::line::LogLine;
use crate::redact;

/// Borrowed diagnostic fields gathered by the caller (the `qbz` bin) for the header.
pub struct DiagFields<'a> {
    pub app_version: &'a str,
    pub os: &'a str,
    pub arch: &'a str,
    pub desktop: &'a str,
    pub session: &'a str,
    pub audio_backend: &'a str,
    pub locale: &'a str,
    pub log_level: &'a str,
}

/// Format a GitHub-ready diagnostics bundle: a `<details>` wrapper, a bold field header,
/// and a log-language fenced block of the last `max_lines.min(last_lines.len())` lines.
pub fn format_diagnostics_bundle(
    f: &DiagFields,
    last_lines: &[LogLine],
    max_lines: usize,
) -> String {
    format_diagnostics_bundle_with_report(f, "", last_lines, max_lines)
}

/// [`format_diagnostics_bundle`] with a full diagnostics REPORT injected between
/// the eight-field header and the log block.
///
/// This exists because the eight fields above are all a frontend can produce on
/// its own, and a bug report wants the ~70 the Diagnostics panel gathers (kernel,
/// distro, install method, GPU, exclusive-mode / DAC / sample-rate state,
/// playback context). The caller renders that markdown — this only wraps it, so
/// the `<details><summary>` collapsible that makes the paste readable on GitHub
/// survives. Pass `""` for no report; [`format_diagnostics_bundle`] is exactly
/// that call.
pub fn format_diagnostics_bundle_with_report(
    f: &DiagFields,
    report: &str,
    last_lines: &[LogLine],
    max_lines: usize,
) -> String {
    let n = max_lines.min(last_lines.len());
    let start = last_lines.len() - n;
    let tail = &last_lines[start..];

    let mut out = String::new();
    out.push_str("<details><summary>qbz diagnostics</summary>\n\n");
    out.push_str(&format!("**App:** qbz v{}\n", f.app_version));
    out.push_str(&format!(
        "**OS:** {} {}  **Desktop:** {} ({})\n",
        f.os, f.arch, f.desktop, f.session
    ));
    out.push_str(&format!("**Audio backend:** {}\n", f.audio_backend));
    out.push_str(&format!(
        "**Locale:** {}  **Log level:** {}\n",
        f.locale, f.log_level
    ));
    if !report.trim().is_empty() {
        out.push('\n');
        out.push_str(report.trim_end());
        out.push('\n');
    }
    out.push_str("\n```log\n");
    for line in tail {
        out.push_str(&format!(
            "{} {} {} {}\n",
            line.format_ts(),
            line.level_str(),
            line.target,
            // Defensive: the ring is already redacted, but never trust the input here.
            redact::redact(&line.message)
        ));
    }
    out.push_str("```\n");
    out.push_str("</details>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Level;

    fn fields() -> DiagFields<'static> {
        DiagFields {
            app_version: "1.2.3",
            os: "linux",
            arch: "x86_64",
            desktop: "KDE",
            session: "wayland",
            audio_backend: "pipewire",
            locale: "en",
            log_level: "info",
        }
    }

    #[test]
    fn bundle_shape_and_redaction() {
        let lines = vec![LogLine {
            ts: 1_700_000_000_000,
            level: Level::Info,
            target: "qbz::net".into(),
            message: "GET /track user_auth_token=SEKRET_TOKEN_123".into(),
        }];
        let out = format_diagnostics_bundle(&fields(), &lines, 200);

        assert!(out.contains("qbz v"), "missing version header: {out}");
        assert!(out.contains("```log"), "missing fenced log block: {out}");
        assert!(out.contains("<details>"), "missing details wrapper: {out}");
        assert!(
            !out.contains("SEKRET_TOKEN_123"),
            "secret leaked into bundle: {out}"
        );
    }

    #[test]
    fn caps_to_max_lines() {
        let lines: Vec<LogLine> = (0..10)
            .map(|i| LogLine {
                ts: i,
                level: Level::Debug,
                target: "t".into(),
                message: format!("line {i}"),
            })
            .collect();
        let out = format_diagnostics_bundle(&fields(), &lines, 3);
        // Only the last 3 lines (7, 8, 9) should appear.
        assert!(out.contains("line 9"));
        assert!(out.contains("line 7"));
        assert!(!out.contains("line 6"));
    }

    #[test]
    fn report_lands_between_header_and_log_block() {
        let lines = vec![LogLine {
            ts: 1,
            level: Level::Info,
            target: "t".into(),
            message: "hello".into(),
        }];
        let report = "# qbz diagnostics\n\n- **Kernel:** 7.1.6\n";
        let out = format_diagnostics_bundle_with_report(&fields(), report, &lines, 200);

        let header = out.find("**Audio backend:**").expect("header");
        let body = out.find("- **Kernel:**").expect("report body");
        let block = out.find("```log").expect("log block");
        assert!(header < body && body < block, "wrong order: {out}");
        // The collapsible survives — that is the whole reason this wraps
        // rather than replaces.
        assert!(out.contains("<details><summary>qbz diagnostics</summary>"));
        assert!(out.trim_end().ends_with("</details>"));
    }

    #[test]
    fn empty_report_is_byte_identical_to_the_plain_bundle() {
        let lines = vec![LogLine {
            ts: 1,
            level: Level::Info,
            target: "t".into(),
            message: "hello".into(),
        }];
        assert_eq!(
            format_diagnostics_bundle(&fields(), &lines, 200),
            format_diagnostics_bundle_with_report(&fields(), "   \n ", &lines, 200)
        );
    }
}
