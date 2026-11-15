import type { Edit } from "./format";

/**
 * Applies an `Edit` to a textarea through the browser's editing command, so the text field's own undo stack keeps
 * working (Ctrl+Z after a toolbar action restores the text). Where `execCommand` is missing or refuses (jsdom, a
 * browser that dropped it) the text is replaced directly and `onValue` is told, so state never drifts.
 */
export function applyEdit(ta: HTMLTextAreaElement, edit: Edit, onValue: (v: string) => void): void {
  ta.focus();
  ta.setSelectionRange(edit.from, edit.to);
  let done = false;
  try {
    done = edit.insert === "" ? document.execCommand("delete") : document.execCommand("insertText", false, edit.insert);
  } catch {
    done = false;
  }
  if (!done) {
    ta.setRangeText(edit.insert, edit.from, edit.to, "end");
    onValue(ta.value);
  }
  ta.setSelectionRange(edit.selStart, edit.selEnd);
}
