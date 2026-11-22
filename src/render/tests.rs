use std::path::PathBuf;

use super::*;

/// Shadows the glob-imported `render` so assertions see plain output.
fn render(md: &str, cfg: &Config) -> String {
    super::render(md, cfg).into_ansi()
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
        overflow: false,
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
        overflow: false,
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
        overflow: false,
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
    assert_eq!(out, "cat\n");
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
    assert!(out.contains("\x1b_Gi=1,a=T,U=1,f=100,t=d,q=2,c=2,r=2,"));
}

#[test]
fn take_uploads_should_send_only_images_on_given_lines_once() {
    let dir = std::env::temp_dir().join(format!("martty-lazy-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok();
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend_from_slice(&[0, 0, 0, 20, 0, 0, 0, 40]);
    std::fs::write(dir.join("a.png"), &png).ok();
    let cfg = Config {
        base_dir: dir,
        ..images_config()
    };
    let filler = "text\n\n".repeat(30);
    let mut doc = super::render(&format!("![a](a.png)\n\n{filler}![b](a.png)"), &cfg);
    std::fs::remove_dir_all(&cfg.base_dir).ok();
    assert!(doc.preamble.is_empty());
    let first = doc.take_uploads(0..10);
    let again = doc.take_uploads(0..10);
    let rest = doc.take_uploads(0..usize::MAX);
    assert!(
        first.contains("i=1,") && !first.contains("i=2,"),
        "{first:?}"
    );
    assert_eq!(again, "");
    assert!(rest.contains("i=2,") && !rest.contains("i=1,"), "{rest:?}");
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
    assert!(out.contains("\x1b_Gi=1,a=T,U=1,f=32,o=z,"));
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
    assert!(out.contains(",a=T,U=1,"));
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
    assert_eq!(strip_ansi(&render(md, &images_config())), "logo\n");
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

fn diagrams_config(center: bool) -> Config {
    Config {
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
    }
}

fn diagram_indent(center: bool) -> Option<usize> {
    let cfg = diagrams_config(center);
    let doc = super::render("```mermaid\nflowchart LR\n  A --> B\n```", &cfg);
    let row = doc.lines.iter().find_map(|l| l.image)?;
    assert!(row.cols < 200, "a small diagram should not fill the width");
    Some(row.prefix_len)
}

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

fn math_config() -> Config {
    Config {
        math: Some(MathImages {
            cell: (10, 24),
            color: (255, 255, 255),
        }),
        ..plain_config()
    }
}

#[test]
fn render_should_place_inline_formula_within_its_line() {
    let doc = super::render("a $x^2$ b", &math_config());
    assert_eq!(doc.image_ids.len(), 1);
    assert_eq!(doc.lines.len(), 1);
    let line = strip_ansi(&doc.lines[0].text);
    assert!(line.starts_with("a \u{10EEEE}"), "{line:?}");
    assert!(line.ends_with(" b"), "{line:?}");
}

#[test]
fn render_should_put_display_formula_on_its_own_rows() {
    let doc = super::render("before\n$$\\frac{a}{b}$$\nafter", &math_config());
    let plain: Vec<String> = doc.lines.iter().map(|l| strip_ansi(&l.text)).collect();
    assert_eq!(plain.first().map(String::as_str), Some("before"));
    assert_eq!(plain.last().map(String::as_str), Some("after"));
    let rows = doc.lines.iter().filter(|l| l.image.is_some()).count();
    assert!(rows >= 2, "{plain:?}");
}

#[test]
fn render_should_show_unknown_formula_as_code() {
    let doc = super::render("$\\notacommand$", &math_config());
    assert!(doc.image_ids.is_empty());
    assert_eq!(strip_ansi(&doc.into_ansi()), "\\notacommand\n");
}

#[test]
fn render_should_put_code_language_at_the_right_of_the_panel() {
    let cfg = Config {
        width: 20,
        code_bg: Some(Color::Ansi(0)),
        ..plain_config()
    };
    let lines: Vec<String> = strip_ansi(&render("```rust\nfn x() {}\n```", &cfg))
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        lines,
        [
            "               rust ",
            " fn x() {}          ",
            "                    "
        ]
    );
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

#[test]
fn render_should_leave_wide_code_unwrapped_with_overflow() {
    let cfg = Config {
        width: 20,
        overflow: true,
        ..plain_config()
    };
    let doc = super::render("```\nlet long_name = some_function(argument);\n```", &cfg);
    assert_eq!(doc.lines.len(), 1);
    assert_eq!(
        doc.lines[0].wide,
        Some(Wide {
            cols: 42,
            visible: 20
        })
    );
}

#[test]
fn render_should_keep_table_columns_natural_with_overflow() {
    let md = "| a | b |\n|---|---|\n| one two three four | five six seven eight |";
    let cfg = Config {
        width: 20,
        overflow: true,
        ..plain_config()
    };
    let doc = super::render(md, &cfg);
    // Top border, header, rule, one unwrapped row, bottom border.
    assert_eq!(doc.lines.len(), 5);
    assert!(doc.lines.iter().all(|l| l.wide.is_some()));
}

#[test]
fn render_should_record_code_source_and_heading_levels() {
    let doc = super::render("## Title\n\n```rust\nfn main() {}\n```", &plain_config());
    assert_eq!(doc.headings[0].level, 2);
    assert_eq!(doc.headings[0].text, "Title");
    let code = &doc.sources[0];
    assert!(code.code);
    assert_eq!(code.text, "fn main() {}");
    assert!(code.lines.contains(&(doc.lines.len() - 1)));
}

#[test]
fn render_should_record_quote_prefix_length() {
    let doc = super::render("> quoted", &plain_config());
    let line = &doc.lines[0];
    assert!(line.text[..line.prefix_len].contains('▎'));
    assert_eq!(strip_ansi(&line.text[line.prefix_len..]), "quoted");
}

#[test]
fn render_should_name_missing_image_without_alt_by_file() {
    let out = strip_ansi(&render("![](img/cat.png?v=2)", &images_config()));
    assert_eq!(out, "cat.png\n");
}

/// `md` rendered with a 20×40 px PNG at `a.png`, images as `#` per row.
fn render_with_png(md: &str) -> Vec<String> {
    let dir = std::env::temp_dir().join(format!("martty-png-{}-{}", std::process::id(), md.len()));
    std::fs::create_dir_all(&dir).ok();
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend_from_slice(&[0, 0, 0, 20, 0, 0, 0, 40]);
    std::fs::write(dir.join("a.png"), &png).ok();
    let cfg = Config {
        base_dir: dir,
        ..images_config()
    };
    let doc = super::render(md, &cfg);
    std::fs::remove_dir_all(&cfg.base_dir).ok();
    doc.lines
        .iter()
        .map(|l| match l.image {
            Some(_) => strip_ansi(&l.text[..l.prefix_len]) + "#",
            None => strip_ansi(&l.text),
        })
        .collect()
}

#[test]
fn render_should_set_html_image_apart_without_its_alt() {
    let lines = render_with_png("- text\n\n  <img src=\"a.png\" alt=\"mockup\">\n\n- next");
    assert_eq!(lines, ["• text", "", "  #", "  #", "", "• next"]);
}

#[test]
fn render_should_put_image_below_its_paragraph_text_without_alt() {
    let lines = render_with_png("Some text ![mockup](a.png) and more.");
    assert_eq!(lines, ["Some text and more.", "", "#", "#"]);
}

#[test]
fn heading_lines_should_match_rendered_headings() {
    let md =
        "---\ntitle: x\n---\n\n# One\n\n```\n# not a heading\n```\n\nTwo\n---\n\n> ### Three\n";
    let doc = super::render(md, &plain_config());
    assert_eq!(heading_lines(md), [5, 11, 14]);
    assert_eq!(doc.headings.len(), 3);
}

#[test]
fn render_should_never_pass_escapes_from_the_document() {
    let md = "a \x1b]52;c;eA==\x07 &#27;]0;t&#7; `\x1b[31m`\n\n```\n\x1b]2;x\x07\n```\n\n<https://e.com/\x1b>\n";
    let out = render(md, &plain_config());
    // The only escapes left are martty's own SGR and OSC 8.
    let ours = |rest: &str| rest.starts_with('[') || rest.starts_with("]8;;");
    assert!(
        out.match_indices('\x1b').all(|(i, _)| ours(&out[i + 1..])),
        "{out:?}"
    );
    assert!(out.contains("␛]52;c;eA==␇"), "{out:?}");
}

/// Two 5×1-cell SVG badges in a fresh directory, as `images_config` sizes them.
fn badges_config() -> Config {
    let dir = std::env::temp_dir().join(format!("martty-badges-{}", std::process::id()));
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="20"/>"#;
    for name in ["a.svg", "b.svg"] {
        std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(dir.join(name), svg))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    Config {
        base_dir: dir,
        ..images_config()
    }
}

#[test]
fn render_should_put_images_of_a_paragraph_side_by_side() {
    let doc = super::render("![a](a.svg)\n![b](b.svg)", &badges_config());
    assert_eq!(doc.lines.len(), 1);
    let line = &doc.lines[0].text;
    assert_eq!(
        line.matches(crate::kitty::PLACEHOLDER).count(),
        10,
        "{line:?}"
    );
    assert_eq!(doc.uploads.len(), 2);
}

#[test]
fn render_should_wrap_images_that_do_not_fit_one_row() {
    let cfg = Config {
        width: 10,
        ..badges_config()
    };
    let doc = super::render("![a](a.svg) ![b](b.svg)", &cfg);
    assert_eq!(doc.lines.len(), 2);
}

#[test]
fn render_should_name_mermaid_error_in_the_panel_label() {
    let cfg = Config {
        width: 60,
        code_bg: Some(Color::Ansi(0)),
        ..diagrams_config(false)
    };
    let out = strip_ansi(&render("```mermaid\nnot a diagram\n```", &cfg));
    let first = out.lines().next().unwrap_or_default();
    assert!(first.trim_start().starts_with("invalid mermaid"), "{out}");
    assert!(
        out.lines()
            .nth(1)
            .is_some_and(|l| l.contains("not a diagram")),
        "{out}"
    );
}
