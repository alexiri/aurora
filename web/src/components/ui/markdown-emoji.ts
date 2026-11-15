/** Lazy extra (FIX-137): github.com emoji shortcodes (`:tada:`). */
import remarkEmoji from "remark-emoji";

const extra = { remark: [[remarkEmoji, { accessible: false }]], rehype: [] };
export default extra;
