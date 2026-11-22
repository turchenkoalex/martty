//! Syntax highlighting with syntect's bundled grammars. Token scopes map to
//! palette colors, so code follows the terminal theme rather than a
//! highlighting theme.

use std::sync::OnceLock;

use syntect::easy::ScopeRangeIterator;
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxSet};
use syntect::util::LinesWithEndings;

use super::span::Span;
use crate::style::{Color, Style};

/// Loading the set is cheap; each grammar compiles its regexes on first
/// use (about 1 ms with Oniguruma).
fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Scope prefixes, their palette color and whether they are italic; the
/// innermost matching scope wins.
fn rules() -> &'static [(Scope, u8, bool)] {
    static RULES: OnceLock<Vec<(Scope, u8, bool)>> = OnceLock::new();
    RULES.get_or_init(|| {
        [
            ("comment", 8, true),
            ("string", 2, false),
            ("constant.numeric", 3, false),
            ("constant.language", 3, false),
            ("constant.character", 3, false),
            ("keyword", 5, false),
            ("storage", 5, false),
            ("entity.name.function", 4, false),
            ("support.function", 4, false),
            ("entity.name", 6, false),
            ("support.type", 6, false),
            ("markup.inserted", 2, false),
            ("markup.deleted", 1, false),
        ]
        .into_iter()
        .filter_map(|(name, color, italic)| Some((Scope::new(name).ok()?, color, italic)))
        .collect()
    })
}

fn style_for(stack: &ScopeStack) -> Style {
    stack
        .as_slice()
        .iter()
        .rev()
        .find_map(|scope| {
            rules()
                .iter()
                .find(|(prefix, ..)| prefix.is_prefix_of(*scope))
        })
        .map(|&(_, color, italic)| Style {
            italic,
            ..Style::fg(Color::Ansi(color))
        })
        .unwrap_or_default()
}

/// Lines of styled spans, tabs expanded, or `None` for an unknown language.
/// Compiles the grammars of `langs` ahead of their first block.
pub(super) fn warm_up(langs: &[String]) {
    for lang in langs {
        let _ = highlight(lang, "x = \"s\" // c\n");
    }
}

pub(super) fn highlight(lang: &str, text: &str) -> Option<Vec<Vec<Span>>> {
    if lang.is_empty() {
        return None;
    }
    let set = syntaxes();
    let syntax = set.find_syntax_by_token(alias(lang))?;
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut lines = Vec::new();
    for line in LinesWithEndings::from(text) {
        let ops = state.parse_line(line, set).ok()?;
        let mut spans = Vec::new();
        for (range, op) in ScopeRangeIterator::new(&ops, line) {
            stack.apply(op).ok()?;
            let piece = line
                .get(range)?
                .trim_end_matches('\n')
                .replace('\t', "    ");
            if !piece.is_empty() {
                spans.push(Span::new(piece, style_for(&stack)));
            }
        }
        lines.push(spans);
    }
    Some(lines)
}

/// Common fence names the bundled grammars don't know, mapped to the
/// closest one they do.
fn alias(lang: &str) -> &str {
    match lang.to_ascii_lowercase().as_str() {
        "typescript" | "ts" | "tsx" | "jsx" | "mjs" | "cjs" => "js",
        "shell" | "zsh" | "console" | "shell-session" => "bash",
        "golang" => "go",
        _ => lang,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colors(lang: &str, text: &str) -> Vec<(String, Option<Color>)> {
        highlight(lang, text)
            .unwrap_or_default()
            .concat()
            .into_iter()
            .map(|s| (s.text, s.style.fg))
            .collect()
    }

    #[test]
    fn highlight_should_color_tokens_from_the_palette() {
        let spans = colors("rust", "fn main() { 42 } // hi");
        let color_of = |text: &str| spans.iter().find(|(t, _)| t == text).and_then(|(_, c)| *c);
        assert_eq!(color_of("fn"), Some(Color::Ansi(5)));
        assert_eq!(color_of("main"), Some(Color::Ansi(4)));
        assert_eq!(color_of("42"), Some(Color::Ansi(3)));
        assert!(
            spans
                .iter()
                .any(|(t, c)| t.contains("hi") && *c == Some(Color::Ansi(8)))
        );
    }

    #[test]
    fn highlight_should_keep_text_and_expand_tabs() {
        let text: String = colors("go", "\tx := 1")
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        assert_eq!(text, "    x := 1");
    }

    #[test]
    fn highlight_should_map_typescript_to_javascript() {
        assert!(highlight("ts", "const x = 1;").is_some());
    }

    #[test]
    fn highlight_should_skip_unknown_or_missing_language() {
        assert!(highlight("nosuchlang", "x").is_none());
        assert!(highlight("", "x").is_none());
    }
}
