//! Vietnamese for the Windows app, from the Mac app's translations
//! (macos/Resources/vi.lproj/Localizable.strings) plus `windows/vi.strings` for text only
//! Windows shows. Every string drawn goes through `tr`: an exact match first, else a key
//! with placeholders (`%@`, `%lld`…) whose fixed parts frame the text ("Freed %@" matches
//! "Freed 1.2 GB"); anything else is drawn as it is (names, paths, sizes).

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

/// Settings › General › Language, in HKCU\Software\OpenCleaner: 0 the system's, 1 English,
/// 2 Vietnamese.
pub const LANG_VALUE: &str = "Language";
static CHOICE: AtomicU8 = AtomicU8::new(0);

pub fn set_choice(choice: u32) {
    CHOICE.store(choice.min(2) as u8, Ordering::Relaxed);
    CACHE.with(|c| c.borrow_mut().clear());
}

pub fn choice() -> u32 {
    CHOICE.load(Ordering::Relaxed).into()
}

pub fn vietnamese() -> bool {
    match CHOICE.load(Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => system_is_vietnamese(),
    }
}

#[cfg(windows)]
fn system_is_vietnamese() -> bool {
    static VI: OnceLock<bool> = OnceLock::new();
    // LANG_VIETNAMESE is 0x2A, the primary language in the low 10 bits
    *VI.get_or_init(|| unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() } & 0x3FF == 0x2A)
}

#[cfg(not(windows))]
fn system_is_vietnamese() -> bool {
    false
}

struct Template {
    /// the fixed text around each placeholder (one more than placeholders)
    parts: Vec<String>,
    value: String,
}

struct Table {
    exact: HashMap<String, String>,
    templates: Vec<Template>,
}

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut exact = HashMap::new();
        for text in [include_str!("../../macos/Resources/vi.lproj/Localizable.strings"), include_str!("../vi.strings")] {
            parse(text, &mut exact);
        }
        let mut templates: Vec<Template> = exact
            .iter()
            .filter(|(k, v)| k != v && k.contains('%'))
            .map(|(k, v)| Template { parts: split(k), value: v.clone() })
            .filter(|t| t.parts.len() > 1 && t.parts.iter().map(|p| p.trim().len()).sum::<usize>() >= 3)
            .collect();
        // the most fixed text first: "Drive %@ is almost full: %@ free" before "%@ free"
        templates.sort_by_key(|t| std::cmp::Reverse(t.parts.iter().map(String::len).sum::<usize>()));
        Table { exact, templates }
    })
}

/// `"key" = "value";` lines, with \" \n \\ escapes; comments and other lines are skipped.
fn parse(text: &str, out: &mut HashMap<String, String>) {
    fn quoted(s: &str) -> Option<(String, &str)> {
        let mut chars = s.strip_prefix('"')?.char_indices();
        let mut value = String::new();
        while let Some((i, c)) = chars.next() {
            match c {
                '\\' => match chars.next()?.1 {
                    'n' => value.push('\n'),
                    't' => value.push('\t'),
                    other => value.push(other),
                },
                '"' => return Some((value, &s[i + 2..])),
                c => value.push(c),
            }
        }
        None
    }
    for line in text.lines() {
        let Some((key, rest)) = quoted(line.trim()) else { continue };
        let Some(rest) = rest.trim_start().strip_prefix('=') else { continue };
        if let Some((value, _)) = quoted(rest.trim_start()) {
            out.insert(key, value);
        }
    }
}

/// The length of a placeholder at the start of `s` ("%@", "%lld", "%1$@"…), if one is there.
fn placeholder(s: &str) -> Option<usize> {
    let rest = s.strip_prefix('%')?;
    let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    let after = &rest[digits..];
    let after = if digits > 0 { after.strip_prefix('$')? } else { after };
    let spec = ["@", "lld", "ld", "llu", "lu", "d", "u", "f"].into_iter().find(|p| after.starts_with(p))?;
    Some(1 + (rest.len() - after.len()) + spec.len())
}

fn split(key: &str) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut i = 0;
    while i < key.len() {
        if let Some(n) = placeholder(&key[i..]) {
            parts.push(String::new());
            i += n;
        } else {
            let c = key[i..].chars().next().unwrap_or(' ');
            parts.last_mut().unwrap().push(c);
            i += c.len_utf8();
        }
    }
    parts
}

/// The values in `text` where `parts` has placeholders, when the fixed parts frame it.
fn capture(parts: &[String], text: &str) -> Option<Vec<String>> {
    let (first, last) = (parts.first()?, parts.last()?);
    let body = text.strip_prefix(first.as_str())?.strip_suffix(last.as_str())?;
    let mut args = Vec::new();
    let mut rest = body;
    for middle in &parts[1..parts.len() - 1] {
        let at = rest.find(middle.as_str())?;
        args.push(rest[..at].to_string());
        rest = &rest[at + middle.len()..];
    }
    args.push(rest.to_string());
    args.iter().all(|a| !a.is_empty()).then_some(args)
}

/// Puts `args` into a translation, in order or where "%2$@" says.
fn fill(value: &str, args: &[String]) -> String {
    let mut out = String::new();
    let (mut i, mut next) = (0, 0);
    while i < value.len() {
        if let Some(n) = placeholder(&value[i..]) {
            let digits: String = value[i + 1..i + n].chars().take_while(char::is_ascii_digit).collect();
            let index = digits.parse::<usize>().map_or_else(|_| { next += 1; next - 1 }, |d| d.saturating_sub(1));
            out.push_str(args.get(index).map_or("", String::as_str));
            i += n;
        } else {
            let c = value[i..].chars().next().unwrap_or(' ');
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

thread_local! {
    static CACHE: RefCell<HashMap<String, Option<String>>> = RefCell::new(HashMap::new());
}

fn translate(text: &str) -> Option<String> {
    let t = table();
    if let Some(v) = t.exact.get(text) {
        return Some(v.clone());
    }
    if let Some(v) = t.templates.iter().find_map(|tpl| capture(&tpl.parts, text).map(|args| fill(&tpl.value, &args))) {
        return Some(v);
    }
    // "Gradle cache, downloaded again when needed · last used 3 days ago": part by part
    let parts: Vec<&str> = text.split(" · ").collect();
    if parts.len() < 2 {
        return None;
    }
    let translated: Vec<Option<String>> = parts.iter().map(|p| translate(p)).collect();
    translated.iter().any(Option::is_some).then(|| {
        parts.iter().zip(translated).map(|(p, t)| t.unwrap_or_else(|| p.to_string())).collect::<Vec<_>>().join(" · ")
    })
}

/// `text` in the chosen language (as it is when English, or when no translation fits).
pub fn tr(text: &str) -> Cow<'_, str> {
    if text.is_empty() || !vietnamese() || !text.chars().any(char::is_alphabetic) {
        return Cow::Borrowed(text);
    }
    let found = CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(hit) = c.get(text) {
            return hit.clone();
        }
        // text with live numbers (a scan's progress) keeps changing: start over now and then
        if c.len() > 4000 {
            c.clear();
        }
        let v = translate(text);
        c.insert(text.to_string(), v.clone());
        v
    });
    found.map_or(Cow::Borrowed(text), Cow::Owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_and_templates() {
        set_choice(2);
        assert_eq!(tr("Settings"), "Cài đặt");
        assert_eq!(tr("Freed 1.2 GB"), "Đã giải phóng 1.2 GB");
        assert_eq!(tr("Drive C: is almost full: 3 GB free"), "Ổ C: sắp đầy: còn trống 3 GB");
        assert_eq!(tr("C:\\Users\\me\\Downloads"), "C:\\Users\\me\\Downloads");
        assert_eq!(tr("4.2 GB"), "4.2 GB");
        assert!(tr("Select everything shown?\n\n12 of these were left unticked on purpose: they may hold settings, history, sign-ins or data an app still uses, or be something OpenCleaner does not recognize. Review them before you remove them. Items moved to the Trash can be put back; items deleted outright cannot.").starts_with("Chọn mọi mục đang hiển thị?\n\n12 mục"));
        assert_eq!(tr("Gradle cache, downloaded again when needed · last used 3 days ago"), "Bộ nhớ đệm của Gradle, sẽ tải lại khi cần · dùng lần cuối 3 ngày trước");
        set_choice(1);
        assert_eq!(tr("Settings"), "Settings");
    }

    #[test]
    fn placeholders() {
        assert_eq!(split("%lld items · %@"), ["", " items · ", ""]);
        assert_eq!(fill("%2$@ của %1$@", &["a".into(), "b".into()]), "b của a");
        let mut m = HashMap::new();
        parse("\"a \\\"b\\\"\" = \"c\\nd\";\n/* x */", &mut m);
        assert_eq!(m.get("a \"b\"").map(String::as_str), Some("c\nd"));
    }
}
