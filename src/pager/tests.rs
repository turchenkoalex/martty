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
fn wheel_should_scroll_three_lines() {
    let mut p = pager(100, 11);
    p.handle(parse_keys(b"\x1b[<65;1;1M").remove(0));
    assert_eq!(p.view().top, 3);
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
fn click_on_diagram_should_open_it() {
    let mut p = pager_with(diagram_doc(), 11);
    let action = p.handle(parse_keys(b"\x1b[<0;3;6M").remove(0));
    assert_eq!(action, Action::Open("graph LR\n A --> B".into()));
}

#[test]
fn click_on_text_should_do_nothing() {
    let mut p = pager_with(diagram_doc(), 11);
    let action = p.handle(parse_keys(b"\x1b[<0;3;2M").remove(0));
    assert_eq!(action, Action::Continue);
}

#[test]
fn z_should_open_visible_diagram() {
    let mut p = pager_with(diagram_doc(), 11);
    assert!(matches!(p.handle(Key::Char('z')), Action::Open(_)));
}

#[test]
fn esc_should_close_fullscreen_diagram() {
    let mut p = pager_with(diagram_doc(), 11);
    p.views
        .push(View::new(text_doc(&["d"]), Some("src".into())));
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
