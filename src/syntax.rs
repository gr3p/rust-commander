use crate::listing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Comment,
    String,
    Keyword,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lang {
    Python,
    Bash,
}

pub fn highlight(name: &str, text: &str) -> Vec<Span> {
    let Some(lang) = language(name, text) else { return Vec::new() };
    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '#' && comment_here(lang, &chars, i) {
            let start = i;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            spans.push(Span { start, end: i, kind: Kind::Comment });
            continue;
        }
        if ch == '"' || ch == '\'' {
            let end = string_end(lang, &chars, i);
            spans.push(Span { start: i, end, kind: Kind::String });
            i = end;
            continue;
        }
        if is_ident_start(ch) {
            let start = i;
            i += 1;
            while i < chars.len() && is_ident(chars[i]) {
                i += 1;
            }
            if keyword(lang, &chars[start..i]) {
                spans.push(Span { start, end: i, kind: Kind::Keyword });
            }
            continue;
        }
        i += 1;
    }
    spans
}

fn language(name: &str, text: &str) -> Option<Lang> {
    match listing::extension(name).to_ascii_lowercase().as_str() {
        "py" | "pyw" => Some(Lang::Python),
        "sh" | "bash" => Some(Lang::Bash),
        _ => shebang(text),
    }
}

fn shebang(text: &str) -> Option<Lang> {
    let line = text.lines().next()?.trim();
    let rest = line.strip_prefix("#!")?;
    let prog = rest.split_whitespace().last()?.rsplit('/').next()?;
    if prog.starts_with("python") {
        Some(Lang::Python)
    } else if prog == "bash" || prog == "sh" {
        Some(Lang::Bash)
    } else {
        None
    }
}

// Bash: # starts a comment only at a token boundary. Python: # anywhere outside a string.
fn comment_here(lang: Lang, chars: &[char], i: usize) -> bool {
    match lang {
        Lang::Python => true,
        Lang::Bash => i == 0 || chars[i - 1].is_whitespace(),
    }
}

fn string_end(lang: Lang, chars: &[char], i: usize) -> usize {
    let quote = chars[i];
    let triple = lang == Lang::Python && i + 2 < chars.len() && chars[i + 1] == quote && chars[i + 2] == quote;
    let mut j = i + if triple { 3 } else { 1 };
    while j < chars.len() {
        let raw_single = lang == Lang::Bash && quote == '\'';
        if chars[j] == '\\' && !raw_single {
            j = (j + 2).min(chars.len());
            continue;
        }
        if triple {
            if j + 2 < chars.len() && chars[j] == quote && chars[j + 1] == quote && chars[j + 2] == quote {
                return j + 3;
            }
        } else if chars[j] == quote {
            return j + 1;
        } else if !triple && chars[j] == '\n' {
            return j;
        }
        j += 1;
    }
    j
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_ident(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn keyword(lang: Lang, word: &[char]) -> bool {
    let mut buf = [0u8; 12];
    if word.len() > buf.len() {
        return false;
    }
    for (i, ch) in word.iter().enumerate() {
        if !ch.is_ascii() {
            return false;
        }
        buf[i] = *ch as u8;
    }
    let word = std::str::from_utf8(&buf[..word.len()]).unwrap_or("");
    match lang {
        Lang::Python => matches!(
            word,
            "False"
                | "None"
                | "True"
                | "and"
                | "as"
                | "assert"
                | "async"
                | "await"
                | "break"
                | "class"
                | "continue"
                | "def"
                | "del"
                | "elif"
                | "else"
                | "except"
                | "finally"
                | "for"
                | "from"
                | "global"
                | "if"
                | "import"
                | "in"
                | "is"
                | "lambda"
                | "nonlocal"
                | "not"
                | "or"
                | "pass"
                | "raise"
                | "return"
                | "try"
                | "while"
                | "with"
                | "yield"
        ),
        Lang::Bash => matches!(
            word,
            "if" | "then" | "else" | "elif" | "fi" | "for" | "while" | "until" | "do" | "done" | "case" | "esac" | "in" | "function"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn painted(name: &str, text: &str) -> Vec<(Kind, String)> {
        highlight(name, text)
            .into_iter()
            .map(|span| {
                let slice: String = text.chars().skip(span.start).take(span.end - span.start).collect();
                (span.kind, slice)
            })
            .collect()
    }

    #[test]
    fn python_marks_keywords_strings_and_comments() {
        let got = painted("a.py", "def f():\n    return 'hi' # c\n");
        assert_eq!(
            got,
            vec![
                (Kind::Keyword, "def".into()),
                (Kind::Keyword, "return".into()),
                (Kind::String, "'hi'".into()),
                (Kind::Comment, "# c".into()),
            ]
        );
    }

    #[test]
    fn string_hides_keywords_and_hashes() {
        let got = painted("a.py", "x = 'def # no'\n");
        assert_eq!(got, vec![(Kind::String, "'def # no'".into())]);
    }

    #[test]
    fn bash_and_shebang() {
        let got = painted("a.sh", "if true; then\n  echo \"hi\" # c\nfi\n");
        assert!(got.contains(&(Kind::Keyword, "if".into())));
        assert!(got.contains(&(Kind::Keyword, "then".into())));
        assert!(got.contains(&(Kind::Keyword, "fi".into())));
        assert!(got.contains(&(Kind::String, "\"hi\"".into())));
        assert!(got.contains(&(Kind::Comment, "# c".into())));
        assert!(!got.iter().any(|(_, word)| word == "echo"));
        let script = painted("run", "#!/usr/bin/env python3\ndef x():\n    pass\n");
        assert!(script.contains(&(Kind::Keyword, "def".into())));
        assert!(painted("notes.txt", "def x():\n").is_empty());
    }
}
