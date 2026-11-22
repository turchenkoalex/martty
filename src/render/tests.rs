use std::path::PathBuf;

use super::*;

/// Shadows the glob-imported `render` so assertions see plain output.
fn render(md: &str, cfg: &Config) -> String {
    super::render(md, cfg).to_ansi()
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Text only, 40 columns: no images, no code background.
fn plain_config() -> Config {
    Config {
        width: 40,
        color: true,
        text_sizing: false,
        code_bg: None,
        images: None,
        headings: None,
        diagrams: None,
        math: None,
        first_image_id: 1,
        base_dir: PathBuf::from("/nonexistent"),
    }
}

fn render_plain(md: &str) -> String {
    strip_ansi(&render(md, &plain_config()))
}

#[test]
fn render_should_indent_list_continuation_lines() {
    let out = render_plain("- one two three four five six seven eight nine ten eleven");
    assert_eq!(
        out,
        "• one two three four five six seven\n  eight nine ten eleven\n"
    );
}

#[test]
fn render_should_separate_paragraphs_with_blank_line() {
    assert_eq!(render_plain("a\n\nb"), "a\n\nb\n");
}

#[test]
fn render_should_not_put_blank_lines_inside_tight_list() {
    assert_eq!(render_plain("- a\n  - b\n- c"), "• a\n  ◦ b\n• c\n");
}

#[test]
fn render_should_keep_padded_inline_code_attached_to_punctuation() {
    let cfg = Config {
        width: 40,
        color: true,
        text_sizing: false,
        code_bg: Some(Color::Ansi(0)),
        images: None,
        headings: None,
        diagrams: None,
        math: None,
        first_image_id: 1,
        base_dir: PathBuf::from("/nonexistent"),
    };
    let out = strip_ansi(&render("a `b c`, d", &cfg));
    assert_eq!(out, "a \u{a0}b c\u{a0}, d\n");
}

fn images_config() -> Config {
    Config {
        width: 40,
        color: true,
        text_sizing: false,
        code_bg: None,
        images: Some(Images {
            cell: (10, 20),
            max_rows: 10,
            zoom: 1.0,
        }),
        headings: None,
        diagrams: None,
        math: None,
        first_image_id: 1,
        base_dir: PathBuf::from("/nonexistent"),
    }
}

#[test]
fn render_should_fall_back_to_alt_text_when_image_missing() {
    let out = strip_ansi(&render("![cat](cat.png)", &images_config()));
    assert_eq!(out, "🖼 cat\n");
}

#[test]
fn render_should_show_png_via_kitty_graphics() {
    let dir = std::env::temp_dir().join(format!("martty-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok();
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend_from_slice(&[0, 0, 0, 20, 0, 0, 0, 40]);
    std::fs::write(dir.join("a.png"), &png).ok();
    let cfg = Config {
        base_dir: dir,
        ..images_config()
    };
    let out = render("![a](a.png)", &cfg);
    std::fs::remove_dir_all(&cfg.base_dir).ok();
    assert!(out.contains("\x1b_Ga=T,U=1,i=1,f=100,t=d,q=2,c=2,r=2,"));
}

fn heading_config() -> Config {
    Config {
        headings: Some(HeadingImages {
            font: HeadingFont::load().unwrap_or_else(|| panic!("font")),
            cell: (10, 20),
            colors: [(255, 0, 255), (0, 0, 255), (0, 255, 255)],
        }),
        ..images_config()
    }
}

#[test]
fn render_should_draw_h1_as_compressed_rgba_image() {
    let out = render("# Hello", &heading_config());
    assert!(out.contains("\x1b_Ga=T,U=1,i=1,f=32,o=z,"));
}

#[test]
fn render_should_emit_image_as_placeholder_lines() {
    let doc = super::render("# Hello", &heading_config());
    let rows = doc.lines.iter().filter(|l| l.image.is_some()).count();
    assert_eq!(rows, 2);
}

#[test]
fn render_should_keep_text_heading_when_glyphs_missing() {
    let out = strip_ansi(&render("# ship 🚀", &heading_config()));
    assert_eq!(out, "ship 🚀\n");
}

#[test]
fn render_should_draw_h3_as_image() {
    let out = render("### Three", &heading_config());
    assert!(out.contains("\x1b_Ga=T,U=1,"));
}

#[test]
fn render_should_show_h4_as_text_without_hashes() {
    let out = strip_ansi(&render("#### Four", &heading_config()));
    assert_eq!(out, "Four\n");
}

#[test]
fn render_should_prefix_quote_lines() {
    assert_eq!(render_plain("> a\n>\n> b"), "▎ a\n▎\n▎ b\n");
}

#[test]
fn render_should_draw_alert_title_inside_quote() {
    assert_eq!(
        render_plain("> [!WARNING]\n> Be careful"),
        "▎ ⚠ Warning\n▎ Be careful\n"
    );
}

#[test]
fn render_should_align_yaml_front_matter() {
    let md = "---\ntitle: \"Hi there\"\ntags: [a, b]\nauthor:\n  name: X\n---\n\nText";
    assert_eq!(
        render_plain(md),
        "title   Hi there\ntags    [a, b]\nauthor\n  name: X\n\nText\n"
    );
}

#[test]
fn render_should_read_toml_front_matter() {
    assert_eq!(render_plain("+++\nkey = 'v'\n+++\n"), "key  v\n");
}

#[test]
fn render_should_right_align_ordered_list_numbers() {
    let md = "8. a\n9. b\n10. c\n    1. x\n    2. y";
    assert_eq!(
        render_plain(md),
        " 8. a\n 9. b\n10. c\n    1. x\n    2. y\n"
    );
}

#[test]
fn render_should_apply_inline_html_tags() {
    let md = "H<sub>2</sub>O x<sup>2</sup> <kbd>Ctrl</kbd> <b>b</b><br>next";
    assert_eq!(render_plain(md), "H₂O x² \u{a0}Ctrl\u{a0} b\nnext\n");
}

#[test]
fn render_should_expand_details_with_summary_marker() {
    let md = "a\n\n<details>\n<summary>More</summary>\n\nHidden\n\n</details>\n\nb";
    assert_eq!(render_plain(md), "a\n\n▾ More\n\nHidden\n\nb\n");
}

#[test]
fn render_should_hide_html_comments_and_unknown_tags() {
    let md = "a\n\n<!-- one\ntwo -->\n\n<p align=\"center\">x &amp; <span>y</span></p>\n\nb";
    assert_eq!(render_plain(md), "a\n\nx & y\n\nb\n");
}

#[test]
fn render_should_close_unbalanced_html_at_block_end() {
    let cfg = images_config();
    let out = render("<b>bold\n\nplain", &cfg);
    let last = out.lines().last().unwrap_or_default();
    assert!(!last.contains(";1"), "{last:?}");
}

#[test]
fn render_should_fall_back_to_html_img_alt() {
    let md = "<p><img src=\"nope.png\" alt=\"logo\"></p>";
    assert_eq!(strip_ansi(&render(md, &images_config())), "🖼 logo\n");
}

#[test]
fn render_should_indent_definitions_under_bold_term() {
    let md = "Term\n: one\n: two\n\nNext";
    assert_eq!(render_plain(md), "Term\n  one\n  two\n\nNext\n");
}

#[test]
fn render_should_highlight_marked_text_only_in_pairs() {
    let cfg = images_config();
    let out = render("a ==b== c == d", &cfg);
    assert_eq!(strip_ansi(&out), "a b c == d\n");
    assert!(out.contains("a \x1b[38;5;0;48;5;3mb\x1b[0m c"), "{out:?}");
}

#[cfg(feature = "mermaid")]
fn diagram_indent(center: bool) -> Option<usize> {
    let cfg = Config {
        width: 200,
        color: true,
        diagrams: Some(Diagrams {
            palette: mermaid::Palette {
                bg: (0, 0, 0),
                fg: (255, 255, 255),
                panel: (30, 30, 30),
                accent: (0, 0, 255),
                ansi: [(255, 0, 0); 6],
            },
            cell: (10, 20),
            max_rows: 20,
            zoom: 1.0,
            max_upscale: 1.0,
            center,
        }),
        ..images_config()
    };
    let doc = super::render("```mermaid\nflowchart LR\n  A --> B\n```", &cfg);
    let row = doc.lines.iter().find_map(|l| l.image)?;
    assert!(row.cols < 200, "a small diagram should not fill the width");
    Some(row.prefix_len)
}

#[cfg(feature = "mermaid")]
#[test]
fn render_should_center_small_diagrams_when_asked() {
    let centered = diagram_indent(true).unwrap_or_default();
    assert!(centered > 0);
    assert_eq!(diagram_indent(false), Some(0));
}

#[test]
fn render_should_keep_line_starting_with_colon_in_paragraph() {
    assert_eq!(
        render_plain("unknown\n:not_an_emoji: and\n::path"),
        "unknown :not_an_emoji: and ::path\n"
    );
}

#[test]
fn render_should_replace_shortcode_at_line_start_after_undoing_definition() {
    assert_eq!(render_plain("ship\n:rocket: now"), "ship 🚀 now\n");
}

#[test]
fn render_should_keep_loose_colon_paragraph_separate() {
    assert_eq!(render_plain("a\n\n:x\n\nb"), "a\n\n:x\n\nb\n");
}

#[test]
fn render_should_undo_fake_definition_inside_list_item() {
    assert_eq!(render_plain("- item\n  :x"), "• item :x\n");
}

#[test]
fn render_should_only_start_definitions_at_colon_and_space() {
    assert_eq!(
        render_plain("T\n: ok\n:not\n\nU\n: two"),
        "T\n  ok :not\nU\n  two\n"
    );
}

#[cfg(feature = "math")]
fn math_config() -> Config {
    Config {
        math: Some(MathImages {
            cell: (10, 24),
            color: (255, 255, 255),
        }),
        ..plain_config()
    }
}

#[cfg(feature = "math")]
#[test]
fn render_should_place_inline_formula_within_its_line() {
    let doc = super::render("a $x^2$ b", &math_config());
    assert_eq!(doc.image_ids.len(), 1);
    assert_eq!(doc.lines.len(), 1);
    let line = strip_ansi(&doc.lines[0].text);
    assert!(line.starts_with("a \u{10EEEE}"), "{line:?}");
    assert!(line.ends_with(" b"), "{line:?}");
}

#[cfg(feature = "math")]
#[test]
fn render_should_put_display_formula_on_its_own_rows() {
    let doc = super::render("before\n$$\\frac{a}{b}$$\nafter", &math_config());
    let plain: Vec<String> = doc.lines.iter().map(|l| strip_ansi(&l.text)).collect();
    assert_eq!(plain.first().map(String::as_str), Some("before"));
    assert_eq!(plain.last().map(String::as_str), Some("after"));
    let rows = doc.lines.iter().filter(|l| l.image.is_some()).count();
    assert!(rows >= 2, "{plain:?}");
}

#[cfg(feature = "math")]
#[test]
fn render_should_show_unknown_formula_as_code() {
    let doc = super::render("$\\notacommand$", &math_config());
    assert!(doc.image_ids.is_empty());
    assert_eq!(strip_ansi(&doc.to_ansi()), "\\notacommand\n");
}

/// Every `tests/fixtures/*.md` rendered as plain text and compared with its
/// snapshot in `tests/snapshots`. Review changes with `cargo insta review`,
/// or accept them with `INSTA_UPDATE=always cargo test`.
#[test]
fn render_should_match_fixture_snapshots() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut fixtures: Vec<PathBuf> = std::fs::read_dir(root.join("fixtures"))
        .unwrap_or_else(|e| panic!("fixtures: {e}"))
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.extension().is_some_and(|e| e == "md"))
        .collect();
    fixtures.sort();
    assert!(!fixtures.is_empty(), "no fixtures found");

    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path(root.join("snapshots"));
    settings.set_prepend_module_to_snapshot(false);
    settings.bind(|| {
        for path in &fixtures {
            let md =
                std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let name = path.file_stem().map(|s| s.to_string_lossy().into_owned());
            let cfg = Config {
                width: 60,
                ..plain_config()
            };
            insta::assert_snapshot!(name, strip_ansi(&render(&md, &cfg)));
        }
    });
}
