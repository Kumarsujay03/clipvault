//! Content classification helpers: code detection and file URI parsing.

/// Heuristic code detection. Scores structural signals; plain prose rarely reaches the bar.
pub fn looks_like_code(text: &str) -> bool {
    let t = text.trim();
    if t.len() < 12 || t.len() > 2_000_000 {
        return false;
    }
    let lines: Vec<&str> = t.lines().take(400).collect();
    let mut score = 0i32;

    const KEYWORDS: &[&str] = &[
        "fn ",
        "let ",
        "const ",
        "function ",
        "def ",
        "class ",
        "import ",
        "from ",
        "#include",
        "return ",
        "public ",
        "private ",
        "struct ",
        "impl ",
        "pub ",
        "async ",
        "await ",
        "package ",
        "interface ",
        "enum ",
        "var ",
        "elif ",
        "SELECT ",
        "#!/",
        "=> ",
        "println!",
        "console.log",
        "echo ",
        "export ",
        "<div",
        "</",
        "func ",
        "using ",
        "namespace ",
    ];
    let kw_hits = KEYWORDS.iter().filter(|k| t.contains(*k)).count() as i32;
    score += kw_hits.min(4);

    let structural = lines
        .iter()
        .filter(|l| {
            let l = l.trim_end();
            l.ends_with(';')
                || l.ends_with('{')
                || l.ends_with('}')
                || l.ends_with("):")
                || l.ends_with("=>")
        })
        .count();
    if structural >= 1 {
        score += 1;
    }
    if lines.len() >= 3 && structural * 3 >= lines.len() {
        score += 2;
    }
    let indented = lines
        .iter()
        .filter(|l| l.starts_with("    ") || l.starts_with('\t'))
        .count();
    if lines.len() >= 3 && indented * 4 >= lines.len() {
        score += 1;
    }
    let symbols = t
        .chars()
        .filter(|c| matches!(c, '{' | '}' | '(' | ')' | ';' | '=' | '<' | '>' | '[' | ']'))
        .count();
    if symbols * 25 >= t.len() {
        score += 1;
    }
    // Prose: mostly sentences ending in periods.
    let sentences = lines.iter().filter(|l| l.trim_end().ends_with('.')).count();
    if sentences * 2 > lines.len() {
        score -= 2;
    }
    score >= 3
}

/// Best-effort language label for a code snippet. Scores characteristic tokens per
/// language and returns the clear winner, or `None` when nothing stands out.
pub fn detect_language(text: &str) -> Option<&'static str> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let sample: String = t.chars().take(20_000).collect();
    let first = sample.lines().next().unwrap_or("").trim();
    if first.starts_with("#!") {
        if first.contains("python") {
            return Some("python");
        }
        if first.contains("node") {
            return Some("javascript");
        }
        if ["sh", "bash", "zsh", "fish"]
            .iter()
            .any(|s| first.ends_with(s) || first.contains(&format!("{s} ")))
        {
            return Some("shell");
        }
    }
    if (sample.starts_with('{') || sample.starts_with('['))
        && serde_json::from_str::<serde_json::Value>(&sample).is_ok()
    {
        return Some("json");
    }
    let lower = sample.to_lowercase();
    if lower.starts_with("<!doctype html") || lower.starts_with("<html") {
        return Some("html");
    }

    let count = |needles: &[&str]| {
        needles
            .iter()
            .map(|n| sample.matches(n).count().min(3))
            .sum::<usize>() as i32
    };
    let starts = |prefixes: &[&str]| {
        sample
            .lines()
            .filter(|l| prefixes.iter().any(|p| l.trim_start().starts_with(p)))
            .count()
            .min(5) as i32
    };
    let mut scores: Vec<(&'static str, i32)> = vec![
        (
            "rust",
            count(&[
                "fn ", "let mut ", "impl ", "pub fn", "::", "println!", "-> ", "&self", "use std",
                "#[derive", "match ",
            ]),
        ),
        (
            "python",
            count(&[
                "def ", "self.", "elif ", "import ", "None", "True", "False", "print(", "__init__",
                "lambda ",
            ]) + starts(&["from ", "@"]),
        ),
        (
            "typescript",
            count(&[
                "interface ",
                ": string",
                ": number",
                ": boolean",
                "export type ",
                "readonly ",
                "as const",
                "<T>",
            ]),
        ),
        (
            "javascript",
            count(&[
                "const ",
                "function ",
                "=> ",
                "console.log",
                "require(",
                "module.exports",
                "===",
                "let ",
            ]),
        ),
        (
            "shell",
            starts(&[
                "$ ", "sudo ", "apt ", "cd ", "ls ", "echo ", "export ", "git ", "npm ", "cargo ",
                "set -", "if [", "for ",
            ]) * 2
                + count(&[" | ", " && ", "$(", "${"]),
        ),
        (
            "sql",
            count(&[
                "SELECT ",
                "FROM ",
                "WHERE ",
                "INSERT INTO",
                "UPDATE ",
                "CREATE TABLE",
                "JOIN ",
                "GROUP BY",
                "select ",
                "from ",
                "where ",
            ]),
        ),
        (
            "html",
            count(&[
                "</div>", "<div", "</span>", "<a href", "</p>", "class=\"", "<br",
            ]),
        ),
        (
            "css",
            count(&[
                "{\n", "; }", "px;", "color:", "margin:", "padding:", "display:", "@media",
            ]),
        ),
        (
            "c",
            count(&[
                "#include", "printf(", "int main", "->", "malloc(", "NULL", "sizeof(",
            ]),
        ),
        (
            "cpp",
            count(&["std::", "cout", "template<", "nullptr", "namespace "]),
        ),
        (
            "java",
            count(&[
                "public class",
                "System.out",
                "public static void",
                "private ",
                "@Override",
                "import java.",
            ]),
        ),
        (
            "go",
            count(&["package ", "func ", ":= ", "fmt.", "err != nil", "go func"]),
        ),
        (
            "yaml",
            starts(&["- ", "---"])
                + sample
                    .lines()
                    .filter(|l| {
                        let l = l.trim();
                        !l.is_empty()
                            && l.split_once(": ")
                                .map(|(k, _)| !k.contains(' ') && !k.is_empty())
                                .unwrap_or(false)
                    })
                    .count()
                    .min(5) as i32,
        ),
    ];
    // TypeScript is a superset of JavaScript: credit shared tokens to it when TS-specific ones appear.
    let js = scores
        .iter()
        .find(|(l, _)| *l == "javascript")
        .map(|(_, s)| *s)
        .unwrap_or(0);
    if let Some(ts) = scores.iter_mut().find(|(l, _)| *l == "typescript") {
        if ts.1 > 0 {
            ts.1 += js;
        }
    }
    let cpp = scores
        .iter()
        .find(|(l, _)| *l == "cpp")
        .map(|(_, s)| *s)
        .unwrap_or(0);
    if cpp > 0 {
        if let Some(c) = scores.iter_mut().find(|(l, _)| *l == "c") {
            c.1 = c.1.min(cpp - 1);
        }
    }
    scores.sort_by_key(|s| std::cmp::Reverse(s.1));
    let (best, top) = scores[0];
    let second = scores[1].1;
    (top >= 2 && top > second).then_some(best)
}

/// Parse `text/uri-list` or `x-special/gnome-copied-files` into (operation, uris).
pub fn parse_uri_list(raw: &str) -> (Option<String>, Vec<String>) {
    let mut op = None;
    let mut uris = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if i == 0 && (line == "copy" || line == "cut") {
            op = Some(line.to_owned());
            continue;
        }
        if line.contains(':') {
            uris.push(line.to_owned());
        }
    }
    (op, uris)
}

/// `file:///home/a%20b.txt` → `/home/a b.txt`. Non-file URIs are returned unchanged.
pub fn uri_to_path(uri: &str) -> String {
    let Some(rest) = uri.strip_prefix("file://") else {
        return uri.to_owned();
    };
    // Strip an optional host component ("file://localhost/...").
    let path = match rest.find('/') {
        Some(0) => rest,
        Some(i) => &rest[i..],
        None => rest,
    };
    percent_decode(path)
}

pub fn path_to_uri(path: &str) -> String {
    if path.contains("://") {
        return path.to_owned();
    }
    let mut out = String::from("file://");
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn file_name(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_code() {
        assert!(looks_like_code("fn main() {\n    println!(\"hi\");\n}"));
        assert!(looks_like_code(
            "import os\n\ndef f(x):\n    return x + 1\n"
        ));
        assert!(looks_like_code("const a = { b: 1 };\nconsole.log(a);"));
        assert!(looks_like_code("SELECT id, name FROM users WHERE id = 1;"));
    }

    #[test]
    fn prose_is_not_code() {
        assert!(!looks_like_code(
            "Meeting moved to Thursday. Please bring the slides."
        ));
        assert!(!looks_like_code("hello world"));
        assert!(!looks_like_code("https://example.com/some/page?x=1"));
        assert!(!looks_like_code(
            "This is a paragraph of text.\nIt has several lines.\nEach line ends with a period."
        ));
    }

    #[test]
    fn detects_languages() {
        let cases = [
            ("fn main() {\n    let mut v = Vec::new();\n    println!(\"{:?}\", v);\n}", "rust"),
            ("import os\n\ndef f(self, x):\n    return None if x else True\n", "python"),
            ("const add = (a, b) => a + b;\nconsole.log(add(1, 2));", "javascript"),
            ("interface User { name: string; age: number }\nexport type Id = string;\nconst u: User = load();", "typescript"),
            ("sudo apt update\nsudo apt install -y curl && echo done", "shell"),
            ("SELECT id, name FROM users WHERE id = 1;", "sql"),
            ("{\"a\": [1, 2, {\"b\": null}]}", "json"),
            ("#include <stdio.h>\nint main(void) {\n    printf(\"hi\");\n    return 0;\n}", "c"),
            ("package main\n\nimport \"fmt\"\n\nfunc main() {\n    x := 1\n    fmt.Println(x)\n}", "go"),
            ("#!/usr/bin/env bash\nset -e\nls", "shell"),
        ];
        for (code, lang) in cases {
            assert_eq!(detect_language(code), Some(lang), "{code}");
        }
        assert_eq!(detect_language("hello"), None);
    }

    #[test]
    fn parses_uri_lists() {
        let (op, uris) = parse_uri_list("copy\nfile:///home/u/a%20b.txt\nfile:///tmp/x\n");
        assert_eq!(op.as_deref(), Some("copy"));
        assert_eq!(uris.len(), 2);
        assert_eq!(uri_to_path(&uris[0]), "/home/u/a b.txt");
        let (op, uris) = parse_uri_list("# comment\r\nfile:///a\r\n");
        assert_eq!((op, uris), (None, vec!["file:///a".to_string()]));
        assert_eq!(uri_to_path("file://localhost/etc/hosts"), "/etc/hosts");
        assert_eq!(uri_to_path("file:///bad%zz"), "/bad%zz");
        assert_eq!(uri_to_path("file:///end%2"), "/end%2");
    }

    #[test]
    fn path_uri_roundtrip() {
        let p = "/home/u/My File (1)#.txt";
        assert_eq!(uri_to_path(&path_to_uri(p)), p);
        assert_eq!(file_name("/a/b/c.txt"), "c.txt");
        assert_eq!(file_name("/a/dir/"), "dir");
    }
}
