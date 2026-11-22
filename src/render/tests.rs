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

fn render_plain(md: &str) -> String {
    let cfg = Config {
        width: 40,
        text_sizing: false,
        code_bg: None,
        images: None,
        headings: None,
        diagrams: None,
        first_image_id: 1,
    };
    strip_ansi(&render(md, &cfg))
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
        text_sizing: false,
        code_bg: Some(Color::Ansi(0)),
        images: None,
        headings: None,
        diagrams: None,
        first_image_id: 1,
    };
    let out = strip_ansi(&render("a `b c`, d", &cfg));
    assert_eq!(out, "a \u{a0}b c\u{a0}, d\n");
}

fn images_config() -> Config {
    Config {
        width: 40,
        text_sizing: false,
        code_bg: None,
        images: Some(Images {
            cell: (10, 20),
            max_rows: 10,
            base_dir: PathBuf::from("/nonexistent"),
        }),
        headings: None,
        diagrams: None,
        first_image_id: 1,
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
    let mut cfg = images_config();
    if let Some(images) = &mut cfg.images {
        images.base_dir = dir;
    }
    let out = render("![a](a.png)", &cfg);
    std::fs::remove_dir_all(
        cfg.images
            .as_ref()
            .map(|i| i.base_dir.clone())
            .unwrap_or_default(),
    )
    .ok();
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
    assert!(out.contains("\x1b[0;38;5;0;48;5;3mb"), "{out:?}");
}
