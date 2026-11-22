//! GitHub-style emoji shortcodes (`:rocket:`). A small table of the common
//! ones; unknown codes are left as they are.

use std::borrow::Cow;

/// Sorted by name for binary search. Symbols that default to text
/// presentation carry VS16 (`\u{fe0f}`) so they draw as emoji, two cells wide.
const EMOJI: &[(&str, &str)] = &[
    ("+1", "👍"),
    ("-1", "👎"),
    ("100", "💯"),
    ("alembic", "⚗\u{fe0f}"),
    ("alien", "👽"),
    ("ambulance", "🚑"),
    ("apple", "🍎"),
    ("arrow_down", "⬇\u{fe0f}"),
    ("arrow_left", "⬅\u{fe0f}"),
    ("arrow_right", "➡\u{fe0f}"),
    ("arrow_up", "⬆\u{fe0f}"),
    ("art", "🎨"),
    ("ballot_box_with_check", "☑\u{fe0f}"),
    ("bar_chart", "📊"),
    ("beer", "🍺"),
    ("bell", "🔔"),
    ("blue_heart", "💙"),
    ("bookmark", "🔖"),
    ("books", "📚"),
    ("boom", "💥"),
    ("bug", "🐛"),
    ("bulb", "💡"),
    ("calendar", "📆"),
    ("camera", "📷"),
    ("chart_with_downwards_trend", "📉"),
    ("chart_with_upwards_trend", "📈"),
    ("clap", "👏"),
    ("clipboard", "📋"),
    ("closed_lock_with_key", "🔐"),
    ("cloud", "☁\u{fe0f}"),
    ("coffee", "☕"),
    ("computer", "💻"),
    ("construction", "🚧"),
    ("cool", "🆒"),
    ("crab", "🦀"),
    ("cry", "😢"),
    ("dart", "🎯"),
    ("email", "📧"),
    ("exclamation", "❗"),
    ("eyes", "👀"),
    ("fire", "🔥"),
    ("gear", "⚙\u{fe0f}"),
    ("ghost", "👻"),
    ("gift", "🎁"),
    ("globe_with_meridians", "🌐"),
    ("green_heart", "💚"),
    ("grin", "😁"),
    ("hammer", "🔨"),
    ("hammer_and_wrench", "🛠\u{fe0f}"),
    ("heart", "❤\u{fe0f}"),
    ("heart_eyes", "😍"),
    ("heavy_check_mark", "✔\u{fe0f}"),
    ("heavy_minus_sign", "➖"),
    ("heavy_plus_sign", "➕"),
    ("hourglass", "⌛"),
    ("house", "🏠"),
    ("information_source", "ℹ\u{fe0f}"),
    ("joy", "😂"),
    ("key", "🔑"),
    ("keyboard", "⌨\u{fe0f}"),
    ("label", "🏷\u{fe0f}"),
    ("laughing", "😆"),
    ("link", "🔗"),
    ("lipstick", "💄"),
    ("lock", "🔒"),
    ("loudspeaker", "📢"),
    ("mag", "🔍"),
    ("memo", "📝"),
    ("moon", "🌙"),
    ("muscle", "💪"),
    ("new", "🆕"),
    ("no_entry", "⛔"),
    ("no_entry_sign", "🚫"),
    ("ok", "🆗"),
    ("ok_hand", "👌"),
    ("package", "📦"),
    ("pencil", "📝"),
    ("penguin", "🐧"),
    ("phone", "☎\u{fe0f}"),
    ("point_right", "👉"),
    ("pray", "🙏"),
    ("pushpin", "📌"),
    ("question", "❓"),
    ("rainbow", "🌈"),
    ("recycle", "♻\u{fe0f}"),
    ("red_circle", "🔴"),
    ("robot", "🤖"),
    ("rocket", "🚀"),
    ("rotating_light", "🚨"),
    ("scream", "😱"),
    ("see_no_evil", "🙈"),
    ("shield", "🛡\u{fe0f}"),
    ("skull", "💀"),
    ("slightly_smiling_face", "🙂"),
    ("smile", "😄"),
    ("smiley", "😃"),
    ("snake", "🐍"),
    ("sparkles", "✨"),
    ("speech_balloon", "💬"),
    ("star", "⭐"),
    ("star2", "🌟"),
    ("sunglasses", "😎"),
    ("sunny", "☀\u{fe0f}"),
    ("sweat_smile", "😅"),
    ("tada", "🎉"),
    ("test_tube", "🧪"),
    ("thinking", "🤔"),
    ("thumbsdown", "👎"),
    ("thumbsup", "👍"),
    ("trophy", "🏆"),
    ("truck", "🚚"),
    ("upside_down_face", "🙃"),
    ("warning", "⚠\u{fe0f}"),
    ("wave", "👋"),
    ("whale", "🐳"),
    ("white_check_mark", "✅"),
    ("wink", "😉"),
    ("wrench", "🔧"),
    ("x", "❌"),
    ("zap", "⚡"),
];

fn lookup(name: &str) -> Option<&'static str> {
    EMOJI
        .binary_search_by_key(&name, |&(n, _)| n)
        .ok()
        .and_then(|i| EMOJI.get(i))
        .map(|&(_, e)| e)
}

pub(super) fn replace_shortcodes(text: &str) -> Cow<'_, str> {
    if !text.contains(':') {
        return Cow::Borrowed(text);
    }
    let is_name =
        |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '+' | '-');
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(':') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after.find(|c: char| !is_name(c)).unwrap_or(after.len());
        let emoji = after[end..]
            .starts_with(':')
            .then(|| lookup(&after[..end]))
            .flatten();
        match emoji {
            Some(emoji) => {
                out.push_str(emoji);
                rest = &after[end + 1..];
            }
            None => {
                out.push(':');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_table_should_be_sorted() {
        assert!(EMOJI.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn replace_shortcodes_should_keep_unknown_codes_and_times() {
        assert_eq!(
            replace_shortcodes("ship :rocket: at 10:30:00 :nope: :+1:"),
            "ship 🚀 at 10:30:00 :nope: 👍"
        );
    }
}
