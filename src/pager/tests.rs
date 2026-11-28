use super::keys::{Key, parse_keys};
use super::overlay::Overlay;
use super::view::View;
use super::*;
use crate::render::{ImageRow, Line};
use unicode_width::UnicodeWidthStr;

fn text_doc(lines: &[&str]) -> Document {
    Document {
        preamble: String::new(),
        uploads: Vec::new(),
        lines: lines.iter().map(|t| Line::new(*t)).collect(),
        image_ids: Vec::new(),
        diagrams: Vec::new(),
        headings: Vec::new(),
        sources: Vec::new(),
    }
}

fn pager_with(doc: Document, rows: usize) -> Pager<'static> {
    let size = Size {
        cols: 80,
        rows,
        cell: None,
    };
    Pager::new("t", size, doc)
}

fn pager(lines: usize, rows: usize) -> Pager<'static> {
    let text: Vec<String> = (0..lines).map(|i| format!("line {i}")).collect();
    let refs: Vec<&str> = text.iter().map(String::as_str).collect();
    pager_with(text_doc(&refs), rows)
}

/// A document whose line 5 is the only row of diagram image 9.
fn diagram_doc() -> Document {
    let mut doc = text_doc(&["a"; 20]);
    doc.lines[5].image = Some(ImageRow {
        id: 9,
        row: 0,
        cols: 10,
        visible: 10,
        prefix_len: 0,
    });
    doc.diagrams.push((9, "graph LR\n A --> B".into()));
    doc
}

#[test]
fn scroll_should_stop_at_last_page() {
    let mut p = pager(100, 11);
    p.handle(Key::End);
    assert_eq!(p.view().top, 90);
}

#[test]
fn arrows_should_page_down_and_up() {
    let mut p = pager(100, 11);
    p.handle(Key::Right);
    assert_eq!(p.view().top, 10);
    p.handle(Key::Left);
    assert_eq!(p.view().top, 0);
}

#[test]
fn scroll_should_not_go_above_top() {
    let mut p = pager(100, 11);
    p.handle(Key::PageUp);
    assert_eq!(p.view().top, 0);
}

#[test]
fn search_should_jump_to_match_smart_case() {
    let mut doc = text_doc(&["x"; 100]);
    doc.lines[42].text = "Hello World".into();
    let mut p = pager_with(doc, 11);
    for key in parse_keys(b"/world\r") {
        p.handle(key);
    }
    assert_eq!(p.view().top, 42);
}

#[test]
fn quit_should_stop_pager() {
    assert_eq!(pager(1, 10).handle(Key::Char('q')), Action::Quit);
}

#[test]
fn zoom_in_should_request_relayout() {
    assert_eq!(pager(1, 10).handle(Key::Char('+')), Action::Relayout);
}

#[test]
fn v_should_open_visible_diagram() {
    let mut p = pager_with(diagram_doc(), 11);
    assert_eq!(p.handle(Key::Char('o')), Action::Open(0));
}

#[test]
fn z_should_page_down_as_in_less() {
    let mut p = pager_with(text_doc(&["x"; 100]), 11);
    p.handle(Key::Char('z'));
    assert_eq!(p.view().top, 10);
}

#[test]
fn esc_should_close_fullscreen_diagram() {
    let mut p = pager_with(diagram_doc(), 11);
    p.views.push(View::new(
        text_doc(&["d"]),
        view::Kind::Diagram("src".into()),
    ));
    assert_eq!(p.handle(Key::Esc), Action::Close);
}

#[test]
fn pan_should_stop_at_max_pan() {
    let mut doc = diagram_doc();
    if let Some(img) = &mut doc.lines[5].image {
        img.cols = 30;
    }
    let mut p = pager_with(doc, 11);
    for _ in 0..10 {
        p.handle(Key::Char('l'));
    }
    assert_eq!(p.view().pan, 20);
}

#[test]
fn draw_should_erase_rows_before_drawing_them() {
    // EL after a full-width line would erase its last column (DEC 7 off).
    let mut frame = String::new();
    pager_with(text_doc(&["abc"]), 5).draw(&mut frame);
    assert!(
        frame.contains("\x1b[1;1H\x1b[0m\x1b[Kabc\x1b[0m"),
        "{frame:?}"
    );
    assert!(!frame.contains("abc\x1b[0m\x1b[K"), "{frame:?}");
}

#[test]
fn draw_should_put_position_at_right_edge_of_status() {
    let mut frame = String::new();
    pager(100, 11).draw(&mut frame);
    assert!(frame.contains(" 1-10/100  10% \x1b[0m"), "{frame:?}");
    let status = frame.rsplit("\x1b[7m").next().unwrap_or_default();
    let status = status.strip_suffix("\x1b[0m\x1b[?25l\x1b[?2026l");
    assert_eq!(status.map(UnicodeWidthStr::width), Some(80), "{frame:?}");
}

#[test]
fn draw_should_keep_position_when_status_is_truncated() {
    let mut p = pager(100, 11);
    p.size.cols = 20;
    let mut frame = String::new();
    p.draw(&mut frame);
    assert!(frame.contains(" 1-10/100  10% \x1b[0m"), "{frame:?}");
}

fn link_line(url: &str, text: &str) -> String {
    format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
}

/// 30 lines with links on lines 2 and 25 and a heading `#far` on line 20.
fn links_doc() -> Document {
    let mut doc = text_doc(&["x"; 30]);
    doc.lines[2].text = link_line("#far", "near");
    doc.lines[25].text = link_line("https://e.com", "web");
    doc.headings.push(heading(20, "far"));
    doc
}

fn heading(line: usize, anchor: &str) -> crate::render::Heading {
    crate::render::Heading {
        line,
        level: 2,
        text: anchor.to_owned(),
        anchor: anchor.to_owned(),
    }
}

fn no_render(_: Layout<'_>, _: Params) -> Document {
    text_doc(&["opened"])
}

#[test]
fn tab_should_select_first_link_on_screen_and_enter_follow_it() {
    let mut p = pager_with(links_doc(), 11);
    assert_eq!(p.handle(Key::Tab), Action::Continue);
    assert_eq!(p.view().selected, Some(0));
    assert_eq!(p.handle(Key::Enter), Action::Follow("#far".into()));
}

#[test]
fn tab_should_scroll_to_link_below_the_screen() {
    let mut p = pager_with(links_doc(), 11);
    p.handle(Key::Tab);
    p.handle(Key::Tab);
    assert_eq!(p.view().selected, Some(1));
    assert!(p.view().top + 10 > 25, "top {}", p.view().top);
}

#[test]
fn esc_should_drop_selection_and_enter_scroll_again() {
    let mut p = pager_with(links_doc(), 11);
    p.handle(Key::Tab);
    p.handle(Key::Esc);
    assert_eq!(p.view().selected, None);
    p.handle(Key::Enter);
    assert_eq!(p.view().top, 1);
}

#[test]
fn follow_anchor_should_scroll_heading_to_top() {
    let mut p = pager_with(links_doc(), 11);
    p.follow("#far", &mut no_render).ok();
    assert_eq!(p.view().top, 20);
}

#[test]
fn follow_markdown_file_should_open_it_and_backspace_return() {
    let path = std::env::temp_dir().join(format!("martty-link-{}.md", std::process::id()));
    std::fs::write(&path, "# hi").ok();
    let mut p = pager_with(links_doc(), 11);
    let url = format!("file://{}", path.display());
    p.follow(&url, &mut no_render).ok();
    std::fs::remove_file(&path).ok();
    assert_eq!(p.views.len(), 2);
    assert!(matches!(p.view().kind, view::Kind::File(_)));
    assert_eq!(p.handle(Key::Backspace), Action::Close);
}

#[test]
fn follow_missing_markdown_file_should_say_so() {
    let mut p = pager_with(links_doc(), 11);
    p.follow("file:///nonexistent/x.md", &mut no_render).ok();
    assert_eq!(p.views.len(), 1);
    assert!(
        p.message
            .as_deref()
            .is_some_and(|m| m.starts_with("No such file"))
    );
}

fn press(p: &mut Pager<'_>, keys: &[u8]) -> Action {
    let mut last = Action::Continue;
    for key in parse_keys(keys) {
        last = p.handle(key);
        p.refresh_hits();
    }
    last
}

#[test]
fn search_should_count_matches_and_step_through_them() {
    let mut doc = text_doc(&["x"; 100]);
    doc.lines[30].text = "foo foo".into();
    doc.lines[60].text = "foo".into();
    let mut p = pager_with(doc, 11);
    press(&mut p, b"/foo\r");
    assert_eq!(p.view().top, 30);
    assert_eq!(p.hit_counter().as_deref(), Some("/foo 1/3"));
    press(&mut p, b"n");
    assert_eq!(p.view().top, 60);
    assert_eq!(p.hit_counter().as_deref(), Some("/foo 3/3"));
    press(&mut p, b"N");
    assert_eq!(p.hit_counter().as_deref(), Some("/foo 1/3"));
}

#[test]
fn search_should_step_to_matches_already_on_screen() {
    let mut doc = text_doc(&["x"; 20]);
    doc.lines[3].text = "foo".into();
    doc.lines[5].text = "foo".into();
    let mut p = pager_with(doc, 11);
    press(&mut p, b"/foo\r");
    press(&mut p, b"n");
    assert_eq!(p.view().current_hit, Some(5));
    assert_eq!(p.view().top, 0);
}

#[test]
fn search_should_mark_matches_and_esc_clear_them() {
    let mut doc = text_doc(&["x"; 20]);
    doc.lines[3].text = "a foo".into();
    let mut p = pager_with(doc, 11);
    press(&mut p, b"/foo\r");
    assert_eq!(p.marked_ranges(3), vec![2..5]);
    press(&mut p, b"\x1b");
    assert!(p.marked_ranges(3).is_empty());
}

#[test]
fn selected_link_should_be_marked_whole_over_a_match_inside_it() {
    let mut p = pager_with(links_doc(), 11);
    press(&mut p, b"/near\r");
    press(&mut p, b"\t");
    let link: Vec<_> = p.view().selected_range(2).into_iter().collect();
    assert_eq!(link.len(), 1);
    assert_eq!(p.marked_ranges(2), link);
}

#[test]
fn search_should_find_headings_drawn_as_images() {
    let mut doc = text_doc(&["x"; 100]);
    doc.lines[10].text = "Images in quotes".into();
    for line in [40, 41] {
        doc.lines[line].text = "\u{10EEEE}".into();
        doc.lines[line].image = Some(ImageRow {
            id: 7,
            row: (line - 40) as u32,
            cols: 6,
            visible: 6,
            prefix_len: 0,
        });
    }
    doc.headings.push(heading(40, "Images"));
    let mut p = pager_with(doc, 11);
    press(&mut p, b"/images\r");
    assert_eq!(p.hit_counter().as_deref(), Some("/images 1/2"));
    press(&mut p, b"n");
    assert_eq!(p.view().top, 40);
    assert_eq!(p.hit_counter().as_deref(), Some("/images 2/2"));
    assert!(p.marked_ranges(40).is_empty());
}

#[test]
fn brackets_should_move_between_headings() {
    let mut doc = text_doc(&["x"; 100]);
    doc.headings = vec![heading(10, "a"), heading(40, "b")];
    let mut p = pager_with(doc, 11);
    press(&mut p, b"]");
    assert_eq!(p.view().top, 10);
    press(&mut p, b"]");
    assert_eq!(p.view().top, 40);
    press(&mut p, b"[");
    assert_eq!(p.view().top, 10);
}

#[test]
fn toc_should_open_at_current_section_and_jump() {
    let mut doc = text_doc(&["x"; 100]);
    doc.headings = vec![heading(10, "a"), heading(40, "b"), heading(70, "c")];
    let mut p = pager_with(doc, 11);
    press(&mut p, b"]]t");
    assert!(matches!(
        p.overlay,
        Some(Overlay::List {
            kind: overlay::List::Toc,
            selected: 1,
            ..
        })
    ));
    press(&mut p, b"j\r");
    assert!(p.overlay.is_none());
    assert_eq!(p.view().top, 70);
}

#[test]
fn o_should_not_open_toc() {
    let mut doc = text_doc(&["x"; 100]);
    doc.headings = vec![heading(10, "a")];
    let mut p = pager_with(doc, 11);
    press(&mut p, b"o");
    assert!(p.overlay.is_none());
}

#[test]
fn marks_should_return_to_saved_line() {
    let mut p = pager(100, 11);
    press(&mut p, b"     ");
    let saved = p.view().top;
    press(&mut p, b"ma");
    press(&mut p, b"G'a");
    assert_eq!(p.view().top, saved);
    press(&mut p, b"''");
    assert_eq!(p.view().top, 90);
}

#[test]
fn help_should_close_on_any_key() {
    let mut p = pager(100, 11);
    press(&mut p, b"?");
    assert!(matches!(p.overlay, Some(Overlay::Help)));
    assert_eq!(press(&mut p, b"q"), Action::Continue);
    assert!(p.overlay.is_none());
}

#[test]
fn copy_should_take_code_block_on_screen() {
    let mut doc = text_doc(&["x"; 20]);
    doc.sources.push(crate::render::Source {
        lines: 4..7,
        text: "let a = 1;".into(),
        code: true,
    });
    let mut p = pager_with(doc, 11);
    assert_eq!(press(&mut p, b"y"), Action::Copy("let a = 1;".into()));
}

#[test]
fn follow_without_file_should_say_so() {
    let mut p = pager(100, 11);
    press(&mut p, b"F");
    assert!(!p.follow);
}

/// 100 lines; lines 10 and 11 are the two rows of an image heading.
fn image_heading_doc() -> Document {
    let mut doc = text_doc(&["x"; 100]);
    for row in 0..2 {
        doc.lines[10 + row as usize].image = Some(ImageRow {
            id: 5,
            row,
            cols: 10,
            visible: 10,
            prefix_len: 0,
        });
    }
    doc.headings.push(heading(10, "h"));
    doc
}

#[test]
fn scroll_should_pass_image_heading_at_top_in_one_step() {
    let mut p = pager_with(image_heading_doc(), 11);
    p.view_mut().top = 10;
    p.handle(Key::Down);
    assert_eq!(p.view().top, 12);
    p.handle(Key::Up);
    assert_eq!(p.view().top, 10);
}

#[test]
fn scroll_should_show_image_heading_at_bottom_whole() {
    // 10 rows: line 9 is the last one shown; one step down would show
    // only the heading's upper half on line 10.
    let mut p = pager_with(image_heading_doc(), 11);
    p.handle(Key::Down);
    assert_eq!(p.view().top, 2);
    p.handle(Key::Up);
    assert_eq!(p.view().top, 0);
}

/// A document with diagrams on lines 5, 40 and 80 (ids 1, 2, 3).
fn diagrams_doc() -> Document {
    let mut doc = text_doc(&["a"; 100]);
    for (n, line) in [5, 40, 80].into_iter().enumerate() {
        let id = u32::try_from(n).unwrap_or(0) + 1;
        doc.lines[line].image = Some(ImageRow {
            id,
            row: 0,
            cols: 10,
            visible: 10,
            prefix_len: 0,
        });
        doc.diagrams.push((id, format!("graph LR\n A{n} --> B")));
    }
    doc
}

#[test]
fn fullscreen_diagram_should_close_on_v_and_space() {
    let mut p = pager_with(diagrams_doc(), 11);
    p.apply(Key::Char('o'), &mut no_render).ok();
    assert!(p.view().is_diagram());
    assert_eq!(p.handle(Key::Char('o')), Action::Close);
    assert_eq!(p.handle(Key::Char(' ')), Action::Close);
}

#[test]
fn arrows_should_switch_diagrams_and_scroll_document_to_them() {
    let mut p = pager_with(diagrams_doc(), 11);
    p.apply(Key::Char('o'), &mut no_render).ok();
    p.apply(Key::Right, &mut no_render).ok();
    p.apply(Key::Right, &mut no_render).ok();
    assert_eq!(p.views.len(), 2);
    assert_eq!(p.view().diagram, Some(2));
    p.apply(Key::Right, &mut no_render).ok();
    assert_eq!(p.view().diagram, Some(2));
    assert!(p.message.as_deref().is_some_and(|m| m.contains("last")));
    p.apply(Key::Left, &mut no_render).ok();
    assert_eq!(p.view().diagram, Some(1));
    p.apply(Key::Esc, &mut no_render).ok();
    let top = p.view().top;
    assert!((top..top + 10).contains(&40), "top {top}");
}

#[test]
fn s_should_lay_the_view_out_as_source_and_back() {
    let mut p = pager(40, 11);
    let mut seen = Vec::new();
    let mut render = |_: Layout<'_>, params: Params| {
        seen.push(params.source);
        text_doc(&["x"; 40])
    };
    p.apply(Key::Char('s'), &mut render).ok();
    assert!(p.view().source);
    // Past the message about the switch, the status line keeps saying so.
    p.message = None;
    assert!(p.status().contains("  source"), "{}", p.status());
    p.apply(Key::Char('s'), &mut render).ok();
    assert!(!p.view().source);
    assert_eq!(seen, [true, false]);
}

#[test]
fn tab_title_should_drop_control_characters() {
    assert_eq!(super::tab_title("a\x1b]2;x\x07b.md"), "a]2;xb.md");
    assert_eq!(super::tab_title(&"x".repeat(500)).len(), 100);
}

#[test]
fn tab_name_should_put_the_first_h1_before_the_name() {
    let titled = |headings: &[(u8, &str)]| {
        let mut doc = text_doc(&["x"]);
        for &(level, text) in headings {
            doc.headings.push(crate::render::Heading {
                level,
                ..heading(0, text)
            });
        }
        pager_with(doc, 5).tab_name(0)
    };
    assert_eq!(
        titled(&[(2, "Intro"), (1, "Guide"), (1, "More")]),
        "Guide · t"
    );
    assert_eq!(titled(&[(2, "Intro")]), "t");
    assert_eq!(titled(&[(1, "T")]), "t", "an H1 that repeats the name");
    assert_eq!(titled(&[(1, "x \u{10EEEE}")]), "t", "an inline formula");
}

/// A pager whose document led through links to `a.md`, `b.md` and `c.md`.
fn deep_pager(cols: usize) -> Pager<'static> {
    let mut p = pager(40, 11);
    p.size.cols = cols;
    for name in ["a.md", "b.md", "c.md"] {
        let kind = view::Kind::File(std::path::PathBuf::from(format!("/docs/{name}")));
        p.views.push(View::new(text_doc(&["x"; 40]), kind));
    }
    p
}

#[test]
fn history_should_go_back_to_the_picked_view() {
    let mut p = deep_pager(80);
    p.views[1].top = 7;
    press(&mut p, b"B");
    assert!(matches!(
        p.overlay,
        Some(Overlay::List {
            kind: overlay::List::History,
            selected: 3,
            ..
        })
    ));
    p.handle(Key::Char('k'));
    p.handle(Key::Char('k'));
    assert_eq!(p.handle(Key::Enter), Action::Return(1));
    p.apply(Key::Char('B'), &mut no_render).ok();
    p.apply(Key::Char('k'), &mut no_render).ok();
    p.apply(Key::Char('k'), &mut no_render).ok();
    p.apply(Key::Enter, &mut no_render).ok();
    assert_eq!(p.views.len(), 2);
    assert_eq!(p.view().top, 7);
}

#[test]
fn history_should_say_when_there_is_nothing_to_go_back_to() {
    let mut p = pager(40, 11);
    press(&mut p, b"B");
    assert!(p.overlay.is_none());
    assert!(p.message.is_some());
}

#[test]
fn search_should_skip_line_prefixes_and_keep_panned_lines_whole() {
    let mut doc = text_doc(&["x"; 5]);
    let prefix = "\x1b[90m▎ \x1b[0m";
    doc.lines[1].text = format!("{prefix}▎abcdefghijklmnopqrstuvwxyz");
    doc.lines[1].prefix_len = prefix.len();
    doc.lines[1].wide = Some(crate::render::Wide {
        cols: 27,
        visible: 10,
    });
    let mut p = pager_with(doc, 11);
    p.view_mut().pan = 3;
    press(&mut p, "/▎\r".as_bytes());
    assert_eq!(p.hit_counter().as_deref(), Some("/▎ 1/1"));
    let mut out = String::new();
    p.draw(&mut out);
    let row = out
        .split("\x1b[2;1H")
        .nth(1)
        .and_then(|r| r.split("\x1b[3;1H").next())
        .unwrap_or_default();
    assert_eq!(super::view::plain_text(row), "▎ cdefghijkl");
}
