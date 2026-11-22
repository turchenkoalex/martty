//! `v`: the file on screen in the user's editor, then back to the pager
//! with the file laid out again.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{ENTER, LEAVE, Layout, Pager, Params, free_images, write_out};
use crate::panics;
use crate::render::{self, Document};
use crate::term::RawMode;

/// How an editor is told the line to open at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineArg {
    /// `+N file`: vi and most terminal editors.
    Plus,
    /// `file:N`: Zed, Sublime Text.
    Suffix,
    /// `-g file:N`: VS Code and its forks.
    Goto,
}

const EDITORS: &[(&str, LineArg)] = &[
    ("vi", LineArg::Plus),
    ("vim", LineArg::Plus),
    ("nvim", LineArg::Plus),
    ("view", LineArg::Plus),
    ("nano", LineArg::Plus),
    ("pico", LineArg::Plus),
    ("micro", LineArg::Plus),
    ("emacs", LineArg::Plus),
    ("emacsclient", LineArg::Plus),
    ("kak", LineArg::Plus),
    ("hx", LineArg::Plus),
    ("helix", LineArg::Plus),
    ("joe", LineArg::Plus),
    ("mg", LineArg::Plus),
    ("ne", LineArg::Plus),
    ("zed", LineArg::Suffix),
    ("zeditor", LineArg::Suffix),
    ("subl", LineArg::Suffix),
    ("code", LineArg::Goto),
    ("code-insiders", LineArg::Goto),
    ("codium", LineArg::Goto),
    ("cursor", LineArg::Goto),
    ("windsurf", LineArg::Goto),
];

impl Pager<'_> {
    pub(super) fn edit(
        &mut self,
        raw: &RawMode,
        render: &mut impl FnMut(Layout<'_>, Params) -> Document,
    ) -> io::Result<()> {
        let Some((depth, path)) = self.editable() else {
            self.message = Some("Nothing to edit: the text came from stdin".into());
            return Ok(());
        };
        let line = if depth == self.views.len() - 1 {
            self.source_line(depth, &path)
        } else {
            None
        };
        let editor = editor();
        let mut out = String::new();
        for view in &self.views {
            out.push_str(&free_images(&view.doc));
        }
        out.push_str(LEAVE);
        write_out(&out)?;
        panics::set_alternate_screen(false);
        let status = {
            let _paused = raw.pause()?;
            command(&editor, line, &path).status()
        };
        // Back on a fresh alternate screen: the images freed above are sent
        // again by laying every view out, which also picks up the edits.
        write_out(ENTER)?;
        panics::set_alternate_screen(true);
        for depth in 0..self.views.len() {
            self.relayout(depth, render)?;
        }
        self.refresh_hits();
        self.message = match status {
            Ok(s) if s.success() => None,
            Ok(s) => Some(format!("{editor}: {s}")),
            Err(e) => Some(format!("{editor}: {e}")),
        };
        Ok(())
    }

    /// The topmost view showing a file, and that file. A full-screen
    /// diagram edits the file it came from.
    fn editable(&self) -> Option<(usize, PathBuf)> {
        (0..self.views.len())
            .rev()
            .find_map(|d| Some((d, self.path(d)?.to_path_buf())))
    }

    /// The source line of the last heading at or above the top of the view.
    fn source_line(&self, depth: usize, path: &Path) -> Option<usize> {
        let view = &self.views[depth];
        let index = view.doc.headings.iter().rposition(|h| h.line <= view.top)?;
        let markdown = std::fs::read_to_string(path).ok()?;
        render::heading_lines(&markdown).get(index).copied()
    }
}

/// `$VISUAL`, then `$EDITOR`, then `vi`, as git and less choose.
fn editor() -> String {
    ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|e| !e.trim().is_empty())
        .unwrap_or_else(|| "vi".into())
}

/// The editor run through `sh`, as git does, so `$EDITOR` may hold
/// arguments (`zed --wait`). The line goes only to editors known to take
/// it, in their own form; others open the file at the top.
fn command(editor: &str, line: Option<usize>, path: &Path) -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(format!("{editor} \"$@\"")).arg(editor);
    let with_line = |line: usize| {
        let mut arg = path.as_os_str().to_owned();
        arg.push(format!(":{line}"));
        arg
    };
    match line.zip(line_arg(editor)) {
        Some((line, LineArg::Plus)) => cmd.arg(format!("+{line}")).arg(path),
        Some((line, LineArg::Suffix)) => cmd.arg(with_line(line)),
        Some((line, LineArg::Goto)) => cmd.arg("-g").arg(with_line(line)),
        None => cmd.arg(path),
    };
    cmd
}

fn line_arg(editor: &str) -> Option<LineArg> {
    let program = editor.split_whitespace().next()?;
    let name = Path::new(program).file_name()?.to_str()?;
    EDITORS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|&(_, arg)| arg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn command_should_pass_line_to_known_editors() {
        let cmd = command("/usr/bin/nvim", Some(12), Path::new("a.md"));
        assert_eq!(
            args(&cmd),
            ["-c", "/usr/bin/nvim \"$@\"", "/usr/bin/nvim", "+12", "a.md"]
        );
    }

    #[test]
    fn command_should_keep_editor_arguments() {
        let cmd = command("zed --wait", Some(12), Path::new("a.md"));
        assert_eq!(
            args(&cmd),
            ["-c", "zed --wait \"$@\"", "zed --wait", "a.md:12"]
        );
    }

    #[test]
    fn command_should_pass_line_to_vs_code_with_goto() {
        let cmd = command("code", Some(3), Path::new("a.md"));
        assert_eq!(args(&cmd)[3..], ["-g", "a.md:3"]);
    }

    #[test]
    fn command_should_open_unknown_editors_at_top() {
        let cmd = command("ed", Some(3), Path::new("a.md"));
        assert_eq!(args(&cmd)[3..], ["a.md"]);
    }

    #[test]
    fn command_should_open_at_top_without_a_line() {
        let cmd = command("vim", None, Path::new("a.md"));
        assert_eq!(args(&cmd), ["-c", "vim \"$@\"", "vim", "a.md"]);
    }
}
