use super::keys::{Key, parse_keys};
use super::view::View;
use super::*;
use crate::render::{ImageRow, Line};

fn text_doc(lines: &[&str]) -> Document {
    Document {
        preamble: String::new(),
        lines: lines
            .iter()
            .map(|t| Line {
                text: (*t).to_owned(),
                image: None,
            })
            .collect(),
        image_ids: Vec::new(),
        diagrams: Vec::new(),
        headings: Vec::new(),
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
fn z_should_open_visible_diagram() {
    let mut p = pager_with(diagram_doc(), 11);
    assert!(matches!(p.handle(Key::Char('z')), Action::Open(_)));
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
        p.handle(Key::Right);
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

fn link_line(url: &str, text: &str) -> String {
    format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
}

/// 30 lines with links on lines 2 and 25 and a heading `#far` on line 20.
fn links_doc() -> Document {
    let mut doc = text_doc(&["x"; 30]);
    doc.lines[2].text = link_line("#far", "near");
    doc.lines[25].text = link_line("https://e.com", "web");
    doc.headings.push(crate::render::Heading {
        line: 20,
        anchor: "far".into(),
    });
    doc
}

fn no_render(_: Layout<'_>, _: Size, _: f32) -> Document {
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
