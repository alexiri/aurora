"use client";

// The text editor of the Files viewer (P5-FILES-3, D-316(f), C-905): CodeMirror 6 in its own lazy chunk (the listing never
// loads it). The editor edits text only; nothing it holds is ever rendered as HTML. Its colours come from the dashboard's
// tokens, so both themes follow; its `<style>` elements carry the page's CSP nonce (no CSP change).
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { HighlightStyle, StreamLanguage, bracketMatching, indentOnInput, syntaxHighlighting, type LanguageSupport } from "@codemirror/language";
import { Compartment, EditorState, type Extension } from "@codemirror/state";
import { EditorView, drawSelection, highlightActiveLine, highlightActiveLineGutter, keymap, lineNumbers } from "@codemirror/view";
import { tags as t } from "@lezer/highlight";
import { useEffect, useRef } from "react";

export interface CodeEditorProps {
  /** The document as the file has it (line breaks per `lineEnding`). */
  initial: string;
  lineEnding: "lf" | "crlf";
  /** The file's base name: selects the language by extension. */
  name: string;
  readOnly: boolean;
  /** Accessible name of the editing area. */
  label: string;
  /** Called with the whole text (line breaks as the file's) after every change. */
  onChange: (text: string) => void;
  /** Ctrl/Cmd+S. */
  onSave: () => void;
}

/** The page's CSP nonce for CodeMirror's `<style>` elements: the one Next put on its scripts (`.nonce` stays readable). */
function pageNonce(): string | undefined {
  const el = document.querySelector<HTMLScriptElement>("script[nonce]");
  return el?.nonce || undefined;
}

/** The dashboard's tokens as an editor theme (both themes: the variables change, not the rules). */
const theme = EditorView.theme({
  "&": { color: "var(--ink)", backgroundColor: "var(--surface)", fontSize: "13px", height: "100%" },
  "&.cm-focused": { outline: "2px solid var(--accent)", outlineOffset: "-2px" },
  ".cm-scroller": { fontFamily: "var(--font-mono, ui-monospace, monospace)", lineHeight: "1.55", overflow: "auto" },
  ".cm-content": { caretColor: "var(--ink)", padding: "8px 0" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--ink)" },
  "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": {
    backgroundColor: "var(--focus-halo)",
  },
  ".cm-gutters": { backgroundColor: "var(--subtle)", color: "var(--muted)", border: "none", borderRight: "1px solid var(--line)" },
  ".cm-activeLine": { backgroundColor: "var(--subtle)" },
  ".cm-activeLineGutter": { backgroundColor: "var(--line)", color: "var(--ink)" },
  ".cm-matchingBracket": { backgroundColor: "var(--accent-soft)", outline: "1px solid var(--accent)" },
});

const highlight = HighlightStyle.define([
  { tag: [t.keyword, t.operatorKeyword, t.modifier, t.controlKeyword], color: "var(--accent-ink)", fontWeight: "600" },
  { tag: [t.string, t.special(t.string), t.regexp], color: "var(--pending-ink)" },
  { tag: [t.number, t.bool, t.null, t.atom], color: "var(--link)" },
  { tag: [t.comment, t.lineComment, t.blockComment], color: "var(--muted)", fontStyle: "italic" },
  { tag: [t.typeName, t.className, t.tagName, t.heading], color: "var(--accent-ink)" },
  { tag: [t.propertyName, t.attributeName, t.definition(t.variableName)], color: "var(--ink)", fontWeight: "500" },
  { tag: [t.invalid], color: "var(--danger-ink)" },
  { tag: [t.link, t.url], color: "var(--link)", textDecoration: "underline" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: t.strong, fontWeight: "700" },
]);

/** Language by extension, loaded on demand (each language is its own small chunk). */
async function languageFor(name: string): Promise<Extension | null> {
  const lower = name.toLowerCase();
  const ext = lower.includes(".") ? lower.slice(lower.lastIndexOf(".") + 1) : "";
  const support = (p: Promise<LanguageSupport>) => p;
  switch (ext) {
    case "js":
    case "mjs":
    case "cjs":
    case "jsx":
      return support(import("@codemirror/lang-javascript").then((m) => m.javascript({ jsx: true })));
    case "ts":
    case "mts":
    case "cts":
    case "tsx":
      return support(import("@codemirror/lang-javascript").then((m) => m.javascript({ jsx: true, typescript: true })));
    case "json":
    case "jsonc":
      return support(import("@codemirror/lang-json").then((m) => m.json()));
    case "md":
    case "markdown":
      return support(import("@codemirror/lang-markdown").then((m) => m.markdown()));
    case "py":
    case "pyi":
      return support(import("@codemirror/lang-python").then((m) => m.python()));
    case "rs":
      return support(import("@codemirror/lang-rust").then((m) => m.rust()));
    case "toml":
      return import("@codemirror/legacy-modes/mode/toml").then((m) => StreamLanguage.define(m.toml));
    case "yml":
    case "yaml":
      return support(import("@codemirror/lang-yaml").then((m) => m.yaml()));
    case "html":
    case "htm":
      return support(import("@codemirror/lang-html").then((m) => m.html()));
    case "css":
      return support(import("@codemirror/lang-css").then((m) => m.css()));
    case "sh":
    case "bash":
    case "zsh":
      return import("@codemirror/legacy-modes/mode/shell").then((m) => StreamLanguage.define(m.shell));
    default:
      return null;
  }
}

/** The text of a state with the file's own line breaks. */
const textOf = (state: EditorState) => state.doc.sliceString(0, state.doc.length, state.lineBreak);

export default function CodeEditor({ initial, lineEnding, name, readOnly, label, onChange, onSave }: CodeEditorProps) {
  const host = useRef<HTMLDivElement>(null);
  // Callbacks change every render; the editor is created once per mount (the viewer remounts it with a new `key`).
  const cb = useRef({ onChange, onSave });
  useEffect(() => {
    cb.current = { onChange, onSave };
  });

  useEffect(() => {
    const parent = host.current;
    if (!parent) return;
    const language = new Compartment();
    const state = EditorState.create({
      doc: initial,
      extensions: [
        EditorState.lineSeparator.of(lineEnding === "crlf" ? "\r\n" : "\n"),
        EditorView.cspNonce.of(pageNonce() ?? ""),
        lineNumbers(),
        highlightActiveLineGutter(),
        history(),
        drawSelection(),
        indentOnInput(),
        bracketMatching(),
        highlightActiveLine(),
        syntaxHighlighting(highlight),
        theme,
        language.of([]),
        EditorState.readOnly.of(readOnly),
        EditorView.editable.of(!readOnly),
        EditorView.contentAttributes.of({ "aria-label": label, "aria-multiline": "true", tabindex: "0" }),
        keymap.of([
          {
            key: "Mod-s",
            preventDefault: true,
            run: () => {
              cb.current.onSave();
              return true;
            },
          },
          indentWithTab,
          ...defaultKeymap,
          ...historyKeymap,
        ]),
        EditorView.updateListener.of((u) => {
          if (u.docChanged) cb.current.onChange(textOf(u.state));
        }),
      ],
    });
    const view = new EditorView({ state, parent });
    let alive = true;
    void languageFor(name)
      .then((ext) => {
        if (alive && ext) view.dispatch({ effects: language.reconfigure(ext) });
      })
      .catch(() => {
        // No highlighting is not an error: the text is still editable.
      });
    return () => {
      alive = false;
      view.destroy();
    };
    // The editor is created once for this document; `key` remounts it for another (Reload).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return <div ref={host} className="h-full min-h-0" data-testid="code-editor" />;
}
