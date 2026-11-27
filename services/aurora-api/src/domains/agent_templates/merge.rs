//! (FIX-251, D-488(b)) The three-way text merge. An update offer that meets an agent's own
//! edit merges **base** (the text last written to the agent), **ours** (the agent's current
//! text) and **theirs** (the new version's text rendered for this agent) instead of offering
//! the all-or-nothing overwrite: a clean merge is applied as it is, a conflict is resolved by
//! the person (**Keep mine**, **Take the new one**, a free edit) before anything is applied.
//!
//! Determinism and bounds: the merge is a pure function of the three texts — the same inputs
//! always produce the same segments, on every server, in every order (no clocks, no maps, no
//! randomness). Inputs above [`MERGE_INPUT_MAX`] characters are refused with a clear error, so
//! a merge can never run unbounded: the callers cap the input at the limits the parts already
//! have (the instructions' and skill bodies' own limits, `FILE_TEXT_MAX` for files).
//!
//! How it works, in one breath: the lines of each text (line endings kept on the line, so
//! CRLF and a missing final newline are part of the line) are diffed against the base with
//! Myers' O(ND) linear-space algorithm; the hunks of both sides are grouped into regions of
//! **overlapping** base ranges (disjoint edits compose unambiguously and merge cleanly, even
//! adjacent ones); a region only one side touched takes that side, a region both touched with
//! the same text takes it once, and a region both touched differently is one conflict hunk —
//! everything the person must decide, and nothing more.

/// The largest text one merge unit takes (`dto::FILE_TEXT_MAX`, the helper file limit the
/// skill files share). The callers refuse above this before merging, with a clear message.
pub(crate) const MERGE_INPUT_MAX: usize = 256 * 1024;

/// The largest free-edit text one conflict hunk accepts: the person's own edit of that hunk,
/// size-limited like an instructions edit (`agents::service::BRIEF_MAX`). The part's own limit
/// is checked again by the caller on the merged result.
pub(crate) const RESOLUTION_TEXT_MAX: usize = 20_000;

/// Why a merge was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeError {
    /// One of the three texts is over [`MERGE_INPUT_MAX`] characters.
    TooLarge,
}

/// Why a resolution set could not be applied to a merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveError {
    /// The resolutions do not match the merge's conflict hunks (a stale review).
    Count,
    /// A free edit is over [`RESOLUTION_TEXT_MAX`] characters.
    TooLarge,
}

/// One conflict hunk: the two texts the person chooses between for this region and nothing
/// else. `ours` or `theirs` may be empty (a deletion on one side).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub ours: String,
    pub theirs: String,
}

/// One piece of the merged text: the parts both sides agree on, and the conflict hunks
/// between them, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Text the merge decided on its own (both sides unchanged, one side changed, or both
    /// sides made the same edit).
    Text(String),
    /// A conflict the person must resolve.
    Conflict(Conflict),
}

/// The merge of one text part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merge {
    segments: Vec<Segment>,
}

/// What the person chose for one conflict hunk. The positions follow the merge's own hunks,
/// the ones the server computed — never a client-side diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    KeepMine,
    TakeTheirs,
    /// The person's own edit of this hunk, as their own text.
    Text(String),
}

impl Merge {
    /// Whether the merge is clean: no conflict hunks, the text applies as it is.
    pub fn clean(&self) -> bool {
        !self
            .segments
            .iter()
            .any(|s| matches!(s, Segment::Conflict(_)))
    }

    /// The conflict hunks in order.
    pub fn conflicts(&self) -> Vec<Conflict> {
        self.segments
            .iter()
            .filter_map(|s| match s {
                Segment::Conflict(c) => Some(c.clone()),
                _ => None,
            })
            .collect()
    }

    /// The number of conflict hunks.
    pub fn conflict_count(&self) -> usize {
        self.segments
            .iter()
            .filter(|s| matches!(s, Segment::Conflict(_)))
            .count()
    }

    /// The merged text of a clean merge. `None` when the merge has conflicts: nothing is
    /// applied to the part until the person resolved them.
    pub fn text(&self) -> Option<String> {
        if !self.clean() {
            return None;
        }
        Some(self.concat_resolved(&[]))
    }

    /// Apply one [`Resolution`] per conflict hunk, in the merge's own order, and return the
    /// merged text. The count must match the merge's hunks (a stale review is refused, never
    /// guessed); a free edit is size-capped. Only the conflict hunks take a resolution — the
    /// auto-merged parts are the server's own and are never overridden by request text.
    pub fn resolve(
        &self,
        resolutions: &[Resolution],
        max_edit_chars: usize,
    ) -> Result<String, ResolveError> {
        if resolutions.len() != self.conflict_count() {
            return Err(ResolveError::Count);
        }
        let mut it = resolutions.iter();
        for s in &self.segments {
            if let Segment::Conflict(_) = s
                && let Some(Resolution::Text(t)) = it.next()
                && t.chars().count() > max_edit_chars
            {
                return Err(ResolveError::TooLarge);
            }
        }
        Ok(self.concat_resolved(resolutions))
    }

    fn concat_resolved(&self, resolutions: &[Resolution]) -> String {
        let mut it = resolutions.iter();
        let mut out = String::new();
        for s in &self.segments {
            match s {
                Segment::Text(t) => out.push_str(t),
                Segment::Conflict(c) => match it.next() {
                    Some(Resolution::KeepMine) => out.push_str(&c.ours),
                    Some(Resolution::TakeTheirs) => out.push_str(&c.theirs),
                    Some(Resolution::Text(t)) => out.push_str(t),
                    // A clean merge resolved with no resolutions: keep ours (the hunk list
                    // was empty, so this arm never runs for a real conflict).
                    None => out.push_str(&c.ours),
                },
            }
        }
        out
    }
}

/// The three-way merge of the base, the agent's current text and the new version's text.
/// The same three texts always produce the same [`Merge`].
pub(crate) fn merge(base: &str, ours: &str, theirs: &str) -> Result<Merge, MergeError> {
    for t in [base, ours, theirs] {
        if t.chars().count() > MERGE_INPUT_MAX {
            return Err(MergeError::TooLarge);
        }
    }
    let b = lines(base);
    let o = lines(ours);
    let t = lines(theirs);
    let ours_hunks = diff_lines(&b, &o);
    let theirs_hunks = diff_lines(&b, &t);

    // Both sides' hunks in base order (ours first on a tie), grouped into regions of
    // overlapping base ranges. Two hunks overlap when the second starts strictly inside the
    // region so far; disjoint edits — even adjacent ones — stay their own regions and merge
    // cleanly.
    #[derive(Clone, Copy)]
    struct Raw {
        side: u8, // 0 = ours, 1 = theirs
        bs: usize,
        be: usize,
        ss: usize,
        se: usize,
    }
    let mut all: Vec<Raw> = Vec::with_capacity(ours_hunks.len() + theirs_hunks.len());
    for (bs, be, ss, se) in &ours_hunks {
        all.push(Raw {
            side: 0,
            bs: *bs,
            be: *be,
            ss: *ss,
            se: *se,
        });
    }
    for (bs, be, ss, se) in &theirs_hunks {
        all.push(Raw {
            side: 1,
            bs: *bs,
            be: *be,
            ss: *ss,
            se: *se,
        });
    }
    all.sort_by_key(|h| (h.bs, h.side));

    let mut regions: Vec<(usize, usize, Vec<Raw>)> = Vec::new();
    for h in all {
        match regions.last_mut() {
            Some((lo, hi, hs)) if h.bs < *hi || (h.bs == *hi && *lo == *hi) => {
                // Overlapping base ranges join, and so do the hunks sitting on the same
                // empty range (two insertions at one point are one place, not two).
                *hi = (*hi).max(h.be);
                hs.push(h);
            }
            _ => regions.push((h.bs, h.be, vec![h])),
        }
    }

    // The text one side contributes to a region: the region's base range mapped into that
    // side's lines — the side's own hunks of the region and the equal lines between them.
    // `off_pre` sums the deltas of the side's hunks entirely before the region (an insertion
    // hunk sitting exactly at `lo` belongs to the region, never to `off_pre`), `own_delta` the
    // deltas of the region's own hunks; the bounds never pass each other, so the slice is the
    // side's honest version of the region.
    fn side_range(all: &[Hunk], lines: &[&str], own: &[Raw], lo: usize, hi: usize) -> String {
        let in_own = |bs: usize, be: usize| own.iter().any(|h| h.bs == bs && h.be == be);
        let mut off_pre = 0isize;
        for (bs, be, ss, se) in all {
            if *bs > lo {
                break;
            }
            if *be <= lo && !in_own(*bs, *be) {
                off_pre += *se as isize - *ss as isize - (*be as isize - *bs as isize);
            }
        }
        let own_delta: isize = own
            .iter()
            .map(|h| h.se as isize - h.ss as isize - (h.be as isize - h.bs as isize))
            .sum();
        let start = (lo as isize + off_pre)
            .min(
                own.iter()
                    .map(|h| h.ss as isize)
                    .min()
                    .unwrap_or(isize::MAX),
            )
            .max(0) as usize;
        let end = (hi as isize + off_pre + own_delta)
            .max(own.iter().map(|h| h.se as isize).max().unwrap_or(0)) as usize;
        lines[start..end].concat()
    }

    let mut segments: Vec<Segment> = Vec::new();
    let mut base_at = 0usize;
    for (lo, hi, hs) in &regions {
        // The unchanged base lines before the region.
        if *lo > base_at {
            segments.push(Segment::Text(b[base_at..*lo].concat()));
        }
        let ours_own: Vec<Raw> = hs.iter().filter(|h| h.side == 0).copied().collect();
        let theirs_own: Vec<Raw> = hs.iter().filter(|h| h.side == 1).copied().collect();
        let o = if ours_own.is_empty() {
            b[*lo..*hi].concat()
        } else {
            side_range(&ours_hunks, &o, &ours_own, *lo, *hi)
        };
        let t = if theirs_own.is_empty() {
            b[*lo..*hi].concat()
        } else {
            side_range(&theirs_hunks, &t, &theirs_own, *lo, *hi)
        };
        match (ours_own.is_empty(), theirs_own.is_empty()) {
            (true, true) => unreachable!("a region is built from at least one hunk"),
            (false, true) => segments.push(Segment::Text(o)),
            (true, false) => segments.push(Segment::Text(t)),
            (false, false) => {
                if o == t {
                    // Both sides made the same edit: take it once.
                    segments.push(Segment::Text(o));
                } else {
                    segments.push(Segment::Conflict(Conflict { ours: o, theirs: t }));
                }
            }
        }
        base_at = *hi;
    }
    if base_at < b.len() {
        segments.push(Segment::Text(b[base_at..].concat()));
    }
    Ok(Merge { segments })
}

// ---- the line diff (Myers, linear space) ---------------------------------------------------------

/// One hunk: `a[bs..be)` is replaced by `b[ss..se)`. The lines carry their endings, so equal
/// lines compare exactly.
type Hunk = (usize, usize, usize, usize);

fn lines(s: &str) -> Vec<&str> {
    let mut v = Vec::new();
    let mut start = 0usize;
    for (i, byte) in s.bytes().enumerate() {
        if byte == b'\n' {
            v.push(&s[start..=i]);
            start = i + 1;
        }
    }
    if start < s.len() {
        v.push(&s[start..]);
    }
    v
}

fn diff_lines(a: &[&str], b: &[&str]) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    diff_rec(a, 0, a.len(), b, 0, b.len(), &mut hunks);
    coalesce(hunks)
}

fn diff_rec(
    a: &[&str],
    mut a0: usize,
    mut a1: usize,
    b: &[&str],
    mut b0: usize,
    mut b1: usize,
    out: &mut Vec<Hunk>,
) {
    // The common prefix and suffix are equal on both sides; never part of a hunk.
    while a0 < a1 && b0 < b1 && a[a0] == b[b0] {
        a0 += 1;
        b0 += 1;
    }
    while a1 > a0 && b1 > b0 && a[a1 - 1] == b[b1 - 1] {
        a1 -= 1;
        b1 -= 1;
    }
    if a0 == a1 {
        if b0 < b1 {
            out.push((a0, a0, b0, b1));
        }
        return;
    }
    if b0 == b1 {
        out.push((a0, a1, b0, b0));
        return;
    }
    match middle_snake(a, a0, a1, b, b0, b1) {
        Some((x, y, u, v)) => {
            // `middle_snake` works in range-relative indices; back to absolute ones.
            let (ax, ay, au, av) = (a0 + x, b0 + y, a0 + u, b0 + v);
            // A meeting point at a corner with nothing traced cannot split the range: one
            // replace hunk keeps the script correct and finite. (Cannot happen for two
            // non-empty trimmed ranges; the guard is the guarantee, not the assumption.)
            if (au == a0 && av == b0) || (au == a1 && av == b1) {
                out.push((a0, a1, b0, b1));
                return;
            }
            diff_rec(a, a0, ax, b, b0, ay, out);
            // The snake (ax, ay)..(au, av) is equal content on both sides: no hunk.
            diff_rec(a, au, a1, b, av, b1, out);
        }
        // Cannot happen for two non-empty ranges (Myers always finds a middle snake); if it
        // ever did, one replace hunk keeps the merge correct and finite.
        None => out.push((a0, a1, b0, b1)),
    }
}

/// Myers' middle snake of `a[a0..a1]` against `b[b0..b1]` (the 1986 paper's linear-space
/// refinement), as `(x, y, u, v)`: the snake runs from `(x, y)` to `(u, v)` in absolute line
/// indices. The reverse search runs as the forward search on the reversed sequences; a
/// forward path of `d` steps overlaps a reverse path of `d'` steps when, on the same absolute
/// diagonal, their reach adds up past the end. For an odd `delta` the overlap is found in the
/// forward loop (against reverse at `d-1`), for an even one in the reverse loop (against
/// forward at `d`) — exactly as in the paper.
fn middle_snake(
    a: &[&str],
    a0: usize,
    a1: usize,
    b: &[&str],
    b0: usize,
    b1: usize,
) -> Option<(usize, usize, usize, usize)> {
    let n = a1 - a0;
    let m = b1 - b0;
    if n == 0 || m == 0 {
        return None;
    }
    let delta = n as isize - m as isize;
    let odd = delta % 2 != 0;
    let max_d = (n + m).div_ceil(2); // the paper's bound for the overlapping half
    let off = max_d as isize; // diagonal k -> index k + off
    let mut vf = vec![0isize; 2 * max_d + 2];
    let mut vr = vec![0isize; 2 * max_d + 2];
    vf[(off + 1) as usize] = 0;
    vr[(off + 1) as usize] = 0;
    for d in 0..=max_d as isize {
        let di = d;
        // Forward, from (0, 0) of the range.
        let mut k = -di;
        while k <= di {
            let ki = (k + off) as usize;
            let mut x: isize = if k == -di || (k != di && vf[ki - 1] < vf[ki + 1]) {
                vf[ki + 1]
            } else {
                vf[ki - 1] + 1
            };
            let mut y = x - k;
            let (sx, sy) = (x, y);
            while (x as usize) < n && (y as usize) < m && a[a0 + x as usize] == b[b0 + y as usize] {
                x += 1;
                y += 1;
            }
            vf[ki] = x;
            if odd && di >= 1 && (k - delta).abs() < di {
                // The reverse search covered this absolute diagonal at d-1.
                let kr = (delta - k + off) as usize;
                if x + vr[kr] >= n as isize {
                    return Some((sx as usize, sy as usize, x as usize, y as usize));
                }
            }
            k += 2;
        }
        // Reverse, as forward over the reversed sequences.
        let mut k = -di;
        while k <= di {
            let ki = (k + off) as usize;
            let mut x: isize = if k == -di || (k != di && vr[ki - 1] < vr[ki + 1]) {
                vr[ki + 1]
            } else {
                vr[ki - 1] + 1
            };
            let mut y = x - k;
            let (sx, sy) = (x, y);
            while (x as usize) < n
                && (y as usize) < m
                && a[a1 - 1 - x as usize] == b[b1 - 1 - y as usize]
            {
                x += 1;
                y += 1;
            }
            vr[ki] = x;
            if !odd && (delta - k).abs() <= di {
                // The forward search covered original diagonal `delta - k` at d.
                let kf = (delta - k + off) as usize;
                if vf[kf] + x >= n as isize {
                    // The snake ran (sx, sy)..(x, y) in reversed coordinates; back to
                    // absolute ones, keeping the original direction.
                    let (ux, uy) = (n as isize - sx, m as isize - sy);
                    let (vx, vy) = (n as isize - x, m as isize - y);
                    return Some((vx as usize, vy as usize, ux as usize, uy as usize));
                }
            }
            k += 2;
        }
    }
    None
}

/// Join hunks of one side that touch (the recursion can emit neighbours at its trim
/// boundaries, and a substitution split into a deletion and an insertion must stay one hunk
/// for the conflict regions to be honest); the region grouping below wants disjoint hunks per
/// side, separated by at least one equal line.
fn coalesce(mut hunks: Vec<Hunk>) -> Vec<Hunk> {
    hunks.sort_by_key(|h| (h.0, h.2));
    let mut out: Vec<Hunk> = Vec::with_capacity(hunks.len());
    for h in hunks {
        match out.last_mut() {
            Some(p) if p.1 >= h.0 => {
                p.1 = p.1.max(h.1);
                p.3 = p.3.max(h.3);
            }
            _ => out.push(h),
        }
    }
    out
}

// -------------------------------------------------------------------------------------------------
// Tests: the merge cases the brief names (FIX-251 Proof), the resolutions, the bounds.
// -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn merged(base: &str, ours: &str, theirs: &str) -> String {
        merge(base, ours, theirs)
            .expect("within the limits")
            .text()
            .expect("clean")
    }

    fn conflicted(base: &str, ours: &str, theirs: &str) -> Merge {
        let m = merge(base, ours, theirs).expect("within the limits");
        assert!(!m.clean(), "expected a conflict: {m:?}");
        m
    }

    #[test]
    fn identical_texts_merge_to_themselves() {
        assert_eq!(merged("a\nb\n", "a\nb\n", "a\nb\n"), "a\nb\n");
        assert_eq!(merged("", "", ""), "");
    }

    #[test]
    fn one_side_unchanged_takes_the_other_whole() {
        assert_eq!(merged("a\nb\n", "a\nb\n", "a\nB\nc\n"), "a\nB\nc\n");
        assert_eq!(merged("a\nb\n", "A\na\nb\nc\n", "a\nb\n"), "A\na\nb\nc\n");
    }

    #[test]
    fn clean_merge_takes_both_edits() {
        assert_eq!(merged("a\nb\nc\n", "a\nB\nc\n", "a\nb\nC\n"), "a\nB\nC\n");
    }

    #[test]
    fn adjacent_edits_merge_cleanly() {
        // Ours edits line 2, theirs edits line 3: disjoint base ranges compose unambiguously.
        assert_eq!(
            merged("1\n2\n3\n4\n", "1\nB\n3\n4\n", "1\n2\nC\n4\n"),
            "1\nB\nC\n4\n"
        );
    }

    #[test]
    fn both_sides_made_the_same_edit_take_it_once() {
        assert_eq!(merged("a\nb\nc\n", "a\nB\nc\n", "a\nB\nc\n"), "a\nB\nc\n");
        // The same insertion at the same place, different text elsewhere.
        assert_eq!(merged("a\nc\n", "a\nx\nc\n", "a\nx\nc\n"), "a\nx\nc\n");
    }

    #[test]
    fn different_edits_of_the_same_line_conflict() {
        let m = conflicted("a\nb\nc\n", "a\nmine\nc\n", "a\ntheirs\nc\n");
        let cs = m.conflicts();
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].ours, "mine\n");
        assert_eq!(cs[0].theirs, "theirs\n");
    }

    #[test]
    fn a_deletion_on_one_side_merges_when_the_other_side_is_elsewhere() {
        assert_eq!(merged("a\nb\nc\n", "a\nc\n", "a\nb\nC\n"), "a\nC\n");
        assert_eq!(merged("a\nb\nc\nd\n", "a\nd\n", "a\nb\nc\nd\n"), "a\nd\n");
    }

    #[test]
    fn a_deletion_against_an_edit_conflicts_with_an_empty_ours() {
        let m = conflicted("a\nb\nc\n", "a\nc\n", "a\nB\nc\n");
        let cs = m.conflicts();
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].ours, "");
        assert_eq!(cs[0].theirs, "B\n");
        // Keep mine: the line stays deleted.
        assert_eq!(
            m.resolve(&[Resolution::KeepMine], RESOLUTION_TEXT_MAX)
                .unwrap(),
            "a\nc\n"
        );
        assert_eq!(
            m.resolve(&[Resolution::TakeTheirs], RESOLUTION_TEXT_MAX)
                .unwrap(),
            "a\nB\nc\n"
        );
    }

    #[test]
    fn crlf_lines_merge_and_keep_their_endings() {
        let base = "a\r\nb\r\nc\r\n";
        assert_eq!(
            merged(base, "a\r\nB\r\nc\r\n", "a\r\nb\r\nC\r\n"),
            "a\r\nB\r\nC\r\n"
        );
        // A changed line ending is a change like any other.
        assert!(merge(base, "a\r\nb\r\nc\r\n", "a\nb\nc\n").is_ok());
    }

    #[test]
    fn a_missing_final_newline_is_part_of_the_last_line() {
        // Theirs adds a final newline, ours is untouched: the clean merge has it.
        assert_eq!(merged("a\nb", "a\nb", "a\nb\n"), "a\nb\n");
        // Ours edited the first line, theirs turned the last line into a proper final
        // newline: different lines, clean.
        assert_eq!(merged("a\nb\nc", "A\nb\nc", "a\nb\nc\n"), "A\nb\nc\n");
        // Both touched the very last line: one conflict, exactly the two last lines.
        let m = conflicted("a\nb", "a\nmine", "a\ntheirs\n");
        let cs = m.conflicts();
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].ours, "mine");
        assert_eq!(cs[0].theirs, "theirs\n");
    }

    #[test]
    fn insertions_on_both_sides_merge_or_conflict_by_place() {
        assert_eq!(
            merged("a\nd\n", "a\nb\nd\n", "a\nd\nc\n"),
            "a\nb\nd\nc\n",
            "insertions at different places compose"
        );
        let m = conflicted("a\nd\n", "a\nmine\nd\n", "a\ntheirs\nd\n");
        assert_eq!(
            m.conflict_count(),
            1,
            "the same place with different texts conflicts"
        );
    }

    #[test]
    fn merging_with_an_unchanged_side_reproduces_the_changed_side() {
        let ours = "x\ny\nz\ny\nx\n";
        assert_eq!(merged("a\nb\nc\n", ours, "a\nb\nc\n"), ours);
        let theirs = "p\nq\nr\n";
        assert_eq!(merged("a\nb\nc\n", "a\nb\nc\n", theirs), theirs);
    }

    #[test]
    fn large_rewrites_conflict_as_one_hunk() {
        let base = "one\ntwo\nthree\n";
        let m = conflicted(base, "A\nB\nC\n", "X\nY\nZ\n");
        assert_eq!(m.conflict_count(), 1);
    }

    #[test]
    fn the_same_inputs_merge_the_same_way_twice() {
        let a = merge("a\nb\nc\nd\ne\n", "a\nB\nc\nd\nE\n", "a\nb\nC\nd\ne\n").unwrap();
        let b = merge("a\nb\nc\nd\ne\n", "a\nB\nc\nd\nE\n", "a\nb\nC\nd\ne\n").unwrap();
        assert_eq!(a, b, "the merge is a pure function of its inputs");
    }

    #[test]
    fn resolutions_apply_in_order_and_only_to_conflicts() {
        let m = conflicted(
            "1\n2\n3\n4\n5\n",
            "1\nmine\n3\nmine too\n5\n",
            "1\ntheirs\n3\ntheirs too\n5\n",
        );
        assert_eq!(m.conflict_count(), 2);
        let text = m
            .resolve(
                &[Resolution::TakeTheirs, Resolution::KeepMine],
                RESOLUTION_TEXT_MAX,
            )
            .unwrap();
        assert_eq!(text, "1\ntheirs\n3\nmine too\n5\n");
        let edited = m
            .resolve(
                &[Resolution::Text("edited\n".into()), Resolution::KeepMine],
                RESOLUTION_TEXT_MAX,
            )
            .unwrap();
        assert_eq!(edited, "1\nedited\n3\nmine too\n5\n");
    }

    #[test]
    fn a_resolution_count_mismatch_is_refused() {
        let m = conflicted("a\nb\n", "a\nmine\n", "a\ntheirs\n");
        assert_eq!(
            m.resolve(&[], RESOLUTION_TEXT_MAX),
            Err(ResolveError::Count),
            "one hunk, no resolution"
        );
        assert_eq!(
            m.resolve(
                &[Resolution::KeepMine, Resolution::KeepMine],
                RESOLUTION_TEXT_MAX
            ),
            Err(ResolveError::Count)
        );
    }

    #[test]
    fn a_free_edit_over_the_limit_is_refused() {
        let m = conflicted("a\nb\n", "a\nmine\n", "a\ntheirs\n");
        let big = "x".repeat(RESOLUTION_TEXT_MAX + 1);
        assert_eq!(
            m.resolve(&[Resolution::Text(big)], RESOLUTION_TEXT_MAX),
            Err(ResolveError::TooLarge)
        );
        let ok = "x".repeat(RESOLUTION_TEXT_MAX);
        assert!(
            m.resolve(&[Resolution::Text(ok)], RESOLUTION_TEXT_MAX)
                .is_ok()
        );
    }

    #[test]
    fn an_input_over_the_limit_is_refused_with_a_clear_error() {
        let big = "x".repeat(MERGE_INPUT_MAX + 1);
        assert_eq!(merge(&big, "a", "a"), Err(MergeError::TooLarge));
        assert_eq!(merge("a", &big, "a"), Err(MergeError::TooLarge));
        assert_eq!(merge("a", "a", &big), Err(MergeError::TooLarge));
    }

    #[test]
    fn empty_and_whitespace_sides_merge() {
        assert_eq!(merged("a\nb\n", "", ""), "");
        assert_eq!(merged("", "a\nb\n", "a\nb\n"), "a\nb\n");
        // One side emptied the text, the other edited it: a conflict with an empty side.
        let m = conflicted("a\nb\n", "", "a\nB\n");
        assert_eq!(m.conflict_count(), 1);
        assert_eq!(m.conflicts()[0].ours, "");
        // One side emptied the text, the other is unchanged: the deletion is taken.
        assert_eq!(merged("a\nb\n", "", "a\nb\n"), "");
    }

    #[test]
    fn repeated_lines_do_not_confuse_the_diff() {
        let base = "x\nx\nx\nx\nx\n";
        assert_eq!(
            merged(base, "x\ny\nx\nx\nx\n", "x\nx\nx\nx\nz\n"),
            "x\ny\nx\nx\nz\n"
        );
        assert_eq!(
            merged(base, base, "x\nx\nx\nx\nx\nx\n"),
            "x\nx\nx\nx\nx\nx\n"
        );
    }

    #[test]
    fn a_renamed_agent_placeholder_flows_through_their_side() {
        // The person edited the greeting; the template renamed the agent on the name line
        // (theirs), which ours never touched: the merge is clean and takes the new name.
        let base = "You are Agent Old.\nBe helpful.\n";
        let ours = "You are Agent Old.\nBe very helpful.\n";
        let theirs = "You are Agent New.\nBe helpful.\n";
        assert_eq!(
            merged(base, ours, theirs),
            "You are Agent New.\nBe very helpful.\n"
        );
    }

    #[test]
    fn hunks_of_one_side_are_disjoint_after_coalescing() {
        let a: Vec<&str> = vec!["a", "b", "c", "d", "e"];
        let b: Vec<&str> = vec!["a", "B", "c", "D", "e"];
        let hunks = diff_lines(&a, &b);
        assert_eq!(hunks, vec![(1, 2, 1, 2), (3, 4, 3, 4)]);
    }
}
