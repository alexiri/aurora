/**
 * Conformance examples (tests/markdown-conformance.test.tsx) whose output differs from the reference
 * HTML, each with the reason. `cm<N>` = CommonMark 0.31.2 example N, `gfm<N>` = GFM 0.29 spec example N.
 * Nothing else may differ: a new difference fails the test.
 */
const HTML = "D-227(3): raw HTML outside GitHub's allowlist is dropped (text kept) and the rest is re-parsed as HTML5, sanitised";
const SCHEME = "D-227(2): only http, https and mailto (and same-origin relative) are links; any other scheme is plain text";
const AUTOLINK = "GFM extended autolinks (the target, D-227(a)): a bare URL or address in text is a link, which CommonMark's own spec says is not";
const REMARK = "remark drops the first blank line of a fenced block (mdast-util-from-markdown strips one leading EOL); cosmetic, no security reason";

export const EXPECTED: Record<string, string> = {
  cm127: REMARK,
  ...Object.fromEntries(
    [150, 156, 157, 158, 163, 169, 170, 171, 172, 173, 176, 178, 187, 201, 344, 476, 477, 491, 494, 524, 536, 613, 614, 615, 616, 617, 626, 630, 631, 642, 643].map((n) => [`cm${n}`, HTML]),
  ),
  gfm652: HTML,
  ...Object.fromEntries([500, 596, 597, 598, 599, 601].map((n) => [`cm${n}`, SCHEME])),
  gfm628: SCHEME,
  ...Object.fromEntries([602, 606, 608, 611, 612].map((n) => [`cm${n}`, AUTOLINK])),
};
