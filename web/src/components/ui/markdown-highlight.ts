/** Lazy extra (FIX-137): syntax highlighting of fenced code (highlight.js "common" languages → React elements). */
import rehypeHighlight from "rehype-highlight";

const extra = {
  remark: [],
  rehype: [[rehypeHighlight, { detect: false, ignoreMissing: true, plainText: ["text", "txt", "plain", "plaintext", "mermaid"] }]],
};
export default extra;
