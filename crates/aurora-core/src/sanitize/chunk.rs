//! Markdown-aware chunking of an agent's long answer (FIX-142 step 3, `13 §5.2`): pieces of at
//! most `max` characters, cut **between top-level blocks**; a block longer than a piece is cut
//! by its kind — a fenced block is closed at the cut and reopened with the same opening line, a
//! table is cut between rows with its header and delimiter rows repeated, a list is cut between
//! items — and only a paragraph (or a single line longer than a piece) is cut inside.

use super::safe::options;
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};
use std::ops::Range;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Fence,
    Table,
    List,
    Other,
}

fn count(s: &str) -> usize {
    s.chars().count()
}

/// Cut `s` at a line (else a space, else anywhere) boundary into pieces of at most `max` chars.
fn cut_text(s: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while count(rest) > max {
        let cut_at = rest.char_indices().nth(max).map_or(rest.len(), |(i, _)| i);
        let head = &rest[..cut_at];
        let split = head
            .rfind("\n\n")
            .or_else(|| head.rfind('\n'))
            .or_else(|| head.rfind(' '))
            .filter(|i| *i > cut_at / 2)
            .unwrap_or(cut_at);
        out.push(rest[..split].trim_matches('\n').to_owned());
        rest = rest[split..].trim_start_matches(['\n', ' ']);
    }
    if !rest.is_empty() {
        out.push(rest.trim_matches('\n').to_owned());
    }
    out
}

/// Pieces `head + lines… + tail`, each within `max`; `head` and `tail` repeat in every piece.
fn cut_lines(head: &str, lines: &[&str], tail: &str, max: usize) -> Vec<String> {
    let overhead = count(head) + count(tail) + 2;
    let room = max.saturating_sub(overhead).max(20);
    let mut out = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut used = 0;
    let flush = |cur: &mut Vec<String>, out: &mut Vec<String>| {
        if cur.is_empty() {
            return;
        }
        let mut p = String::new();
        if !head.is_empty() {
            p.push_str(head);
            p.push('\n');
        }
        p.push_str(&cur.join("\n"));
        if !tail.is_empty() {
            p.push('\n');
            p.push_str(tail);
        }
        out.push(p);
        cur.clear();
    };
    for l in lines {
        // A single line longer than a piece is cut by characters (nothing else can hold it).
        let mut pieces: Vec<String> = Vec::new();
        let mut rest = *l;
        while count(rest) > room {
            let at = rest.char_indices().nth(room).map_or(rest.len(), |(i, _)| i);
            pieces.push(rest[..at].to_owned());
            rest = &rest[at..];
        }
        pieces.push(rest.to_owned());
        for p in pieces {
            let add = count(&p) + 1;
            if !cur.is_empty() && used + add > room {
                flush(&mut cur, &mut out);
                used = 0;
            }
            used += add;
            cur.push(p);
        }
    }
    flush(&mut cur, &mut out);
    out
}

fn fence_marker(opener: &str) -> String {
    let t = opener.trim_start();
    let c = t.chars().next().unwrap_or('`');
    t.chars().take_while(|x| *x == c).collect()
}

/// A block longer than `max`, cut by its kind.
fn split_block(text: &str, kind: Kind, items: &[Range<usize>], max: usize) -> Vec<String> {
    match kind {
        Kind::Fence => {
            let lines: Vec<&str> = text.lines().collect();
            let opener = lines.first().copied().unwrap_or("```");
            let marker = fence_marker(opener);
            let last = lines.last().copied().unwrap_or("");
            let closed = lines.len() > 1
                && last.trim().len() >= marker.len()
                && last.trim().chars().all(|c| marker.starts_with(c));
            let body = &lines[1..lines.len() - usize::from(closed)];
            if body.is_empty() {
                return cut_text(text, max);
            }
            cut_lines(opener, body, &marker, max)
        }
        Kind::Table => {
            let lines: Vec<&str> = text.lines().collect();
            if lines.len() < 3 || count(lines[0]) + count(lines[1]) + 40 > max {
                return cut_lines("", &lines, "", max);
            }
            let head = format!("{}\n{}", lines[0], lines[1]);
            cut_lines(&head, &lines[2..], "", max)
        }
        Kind::List if !items.is_empty() => {
            let mut out = Vec::new();
            let mut cur = String::new();
            let mut at = 0;
            for it in items {
                let piece = &text[at..it.end.min(text.len())];
                at = it.end.min(text.len());
                if count(piece) > max {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                    out.extend(cut_text(piece.trim_matches('\n'), max));
                } else if !cur.is_empty() && count(&cur) + count(piece) > max {
                    out.push(std::mem::take(&mut cur));
                    cur.push_str(piece.trim_start_matches('\n'));
                } else {
                    cur.push_str(piece);
                }
            }
            if !cur.trim().is_empty() {
                out.push(cur);
            }
            out.into_iter()
                .map(|p| p.trim_matches('\n').to_owned())
                .collect()
        }
        _ => cut_text(text, max),
    }
}

/// Split `s` into pieces of at most `max` characters, at Markdown block boundaries (see the
/// module docs). Empty input gives no pieces; the pieces joined by blank lines read as `s`.
pub fn markdown_chunks(s: &str, max: usize) -> Vec<String> {
    let s = s.trim_matches(['\n', ' ', '\t', '\r']);
    if s.is_empty() {
        return Vec::new();
    }
    let max = max.max(40);
    // Top-level blocks: (source range including the gap before it, kind, list items).
    let mut blocks: Vec<(Range<usize>, Kind, Vec<Range<usize>>)> = Vec::new();
    let mut depth = 0usize;
    let mut prev_end = 0usize;
    for (e, r) in Parser::new_ext(s, options()).into_offset_iter() {
        match &e {
            Event::Start(tag) => {
                if depth == 0 {
                    let kind = match tag {
                        Tag::CodeBlock(CodeBlockKind::Fenced(_)) => Kind::Fence,
                        Tag::Table(_) => Kind::Table,
                        Tag::List(_) => Kind::List,
                        _ => Kind::Other,
                    };
                    blocks.push((prev_end..r.end.max(prev_end), kind, Vec::new()));
                    prev_end = r.end.max(prev_end);
                } else if depth == 1
                    && matches!(tag, Tag::Item)
                    && let Some(last) = blocks.last_mut()
                    && last.1 == Kind::List
                {
                    let base = last.0.start;
                    last.2
                        .push(r.start.saturating_sub(base)..r.end.saturating_sub(base));
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            _ if depth == 0 => {
                blocks.push((prev_end..r.end.max(prev_end), Kind::Other, Vec::new()));
                prev_end = r.end.max(prev_end);
            }
            _ => {}
        }
    }
    if prev_end < s.len() {
        if let Some(last) = blocks.last_mut() {
            last.0.end = s.len();
        } else {
            blocks.push((0..s.len(), Kind::Other, Vec::new()));
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for (range, kind, items) in blocks {
        let text = &s[range];
        if count(text.trim_matches('\n')) > max {
            if !cur.trim().is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
            let body = text.trim_matches('\n');
            // The item ranges are relative to the block start (gap included); re-base on `body`.
            let lead = text.len() - text.trim_start_matches('\n').len();
            let rebased: Vec<Range<usize>> = items
                .iter()
                .map(|i| i.start.saturating_sub(lead)..i.end.saturating_sub(lead))
                .collect();
            let mut parts = split_block(body, kind, &rebased, max);
            if let Some(last) = parts.pop() {
                out.extend(parts);
                cur = last;
            }
        } else if !cur.is_empty() && count(&cur) + count(text) > max {
            out.push(std::mem::take(&mut cur));
            cur.push_str(text.trim_start_matches('\n'));
        } else {
            cur.push_str(text);
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out.into_iter()
        .map(|p| p.trim_matches('\n').to_owned())
        .filter(|p| !p.trim().is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fits(v: &[String], max: usize) {
        assert!(
            v.iter().all(|p| count(p) <= max),
            "{:?}",
            v.iter().map(|p| count(p)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn short_text_is_one_piece_and_empty_is_none() {
        assert!(markdown_chunks("  \n", 100).is_empty());
        assert_eq!(markdown_chunks("# T\n\nbody", 100), vec!["# T\n\nbody"]);
    }

    #[test]
    fn cuts_between_blocks_not_inside_them() {
        let a = "a ".repeat(30);
        let src = format!("{a}\n\n- x\n- y\n\n> quote\n\n{a}");
        let v = markdown_chunks(&src, 80);
        fits(&v, 80);
        // Greedy packing: the first paragraph, the list and the quote share a piece; none is cut.
        assert_eq!(v.len(), 2, "{v:?}");
        assert!(v[0].ends_with("- x\n- y\n\n> quote"), "{v:?}");
    }

    #[test]
    fn a_long_fence_is_closed_and_reopened_with_its_info_string() {
        let code: String = (0..40).map(|i| format!("let v{i} = {i};\n")).collect();
        let v = markdown_chunks(&format!("intro\n\n```rust\n{code}```\n\nafter"), 200);
        fits(&v, 200);
        let fences: Vec<&String> = v.iter().filter(|p| p.contains("```rust")).collect();
        assert!(fences.len() >= 3, "{v:?}");
        for p in &fences {
            assert!(
                p.trim_start().starts_with("```rust") || p.contains("\n```rust\n"),
                "{p}"
            );
            assert_eq!(p.matches("```").count() % 2, 0, "unbalanced fence in {p}");
        }
        let body: Vec<&str> = v
            .iter()
            .flat_map(|p| p.lines())
            .filter(|l| l.starts_with("let v"))
            .collect();
        assert_eq!(body.len(), 40, "no code line lost or duplicated");
    }

    #[test]
    fn a_long_table_is_split_by_rows_with_the_header_repeated() {
        let rows: String = (0..60).map(|i| format!("| row{i} | {i} |\n")).collect();
        let v = markdown_chunks(&format!("| name | n |\n|---|--:|\n{rows}"), 160);
        fits(&v, 160);
        assert!(v.len() > 2, "{v:?}");
        for p in &v {
            assert!(
                p.starts_with("| name | n |\n|---|--:|\n"),
                "header missing in {p}"
            );
            // Each piece is a valid table: header, delimiter, then at least one row.
            assert!(p.lines().count() >= 3, "{p}");
        }
        let n: usize = v
            .iter()
            .map(|p| p.lines().filter(|l| l.starts_with("| row")).count())
            .sum();
        assert_eq!(n, 60);
    }

    #[test]
    fn a_long_list_is_split_between_items() {
        let items: String = (1..=50)
            .map(|i| format!("{i}. item number {i}\n"))
            .collect();
        let v = markdown_chunks(&items, 120);
        fits(&v, 120);
        assert!(v.len() > 2);
        for p in &v {
            assert!(p.lines().all(|l| l.starts_with(char::is_numeric)), "{p}");
        }
        let n: usize = v.iter().map(|p| p.lines().count()).sum();
        assert_eq!(n, 50);
    }

    #[test]
    fn one_enormous_line_is_still_cut() {
        let v = markdown_chunks(&"x".repeat(1000), 100);
        fits(&v, 100);
        assert_eq!(v.concat().len(), 1000);
    }

    #[test]
    fn a_nested_list_item_longer_than_a_piece_is_cut_inside() {
        let big = "word ".repeat(100);
        let v = markdown_chunks(&format!("- first\n- {big}\n- last"), 120);
        fits(&v, 120);
        assert!(v.first().is_some_and(|p| p.starts_with("- first")));
        assert!(
            v.last()
                .is_some_and(|p| p.ends_with("- last") || p.contains("last"))
        );
    }
}
