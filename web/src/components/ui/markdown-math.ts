/** Lazy extra (FIX-137): `$…$`, `$$…$$` and ```` ```math ```` — loaded only for messages that contain math. */
import rehypeKatex from "rehype-katex";
import remarkMath from "remark-math";
import "katex/dist/katex.min.css";

import { remarkGithubInlineMath } from "./markdown-pipeline";

const extra = {
  remark: [remarkMath, remarkGithubInlineMath],
  // D-227: KaTeX `trust: false` (no `\href`, `\url`, `\includegraphics`, …); a bad formula shows as its source.
  rehype: [[rehypeKatex, { trust: false, strict: "ignore", throwOnError: false, output: "htmlAndMathml" }]],
};
export default extra;
