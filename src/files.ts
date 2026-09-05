// File open/save UX.
//
// Two quiet ways to destroy someone's work, both guarded here and in Rust:
//
//   * Saving a lossily-decoded buffer over its source. A file that was not
//     valid UTF-8 came back with U+FFFD replacement characters; writing that
//     back discards the original bytes for good. Such tabs refuse Save and
//     offer Save As instead.
//   * Overwriting a file that changed on disk since we opened it. The backend
//     compares mtime and reports a conflict; we ask rather than choosing.

import type { EditorView } from "@codemirror/view";

import { api, type OpenedFile } from "./api";
import type { ScriptTab, TabManager } from "./tabs";

// --------------------------------------------------------------- modal helper

interface Choice<T extends string> {
  value: T;
  label: string;
  primary?: boolean;
  danger?: boolean;
}

/**
 * A modal question. Resolves to the chosen value, or `null` if dismissed —
 * Escape and backdrop dismissal both mean "cancel", never "go ahead".
 */
function choose<T extends string>(
  title: string,
  message: string,
  choices: Choice<T>[],
): Promise<T | null> {
  return new Promise((resolve) => {
    const dlg = document.createElement("dialog");
    dlg.className = "ask";

    const h = document.createElement("h2");
    h.textContent = title;
    const p = document.createElement("p");
    p.textContent = message;
    const menu = document.createElement("menu");

    let settled = false;
    const done = (value: T | null) => {
      if (settled) return;
      settled = true;
      dlg.close();
      dlg.remove();
      resolve(value);
    };

    for (const c of choices) {
      const b = document.createElement("button");
      b.textContent = c.label;
      if (c.primary) b.classList.add("primary");
      if (c.danger) b.classList.add("danger");
      b.onclick = () => done(c.value);
      menu.append(b);
    }

    dlg.append(h, p, menu);
    // Escape closes the dialog without picking anything.
    dlg.addEventListener("cancel", (e) => {
      e.preventDefault();
      done(null);
    });
    document.body.append(dlg);
    dlg.showModal();
    (menu.querySelector("button.primary") ?? menu.querySelector("button"))
      ?.dispatchEvent(new Event("focus"));
    (menu.querySelector("button.primary") as HTMLButtonElement | null)?.focus();
  });
}

// ------------------------------------------------------------------- file ux

export interface FileUxDeps {
  tabs: TabManager;
  view: EditorView;
  /** Show a transient message in the results area. */
  notify: (message: string) => void;
  /** Refresh the per-tab file indicator in the editor pane header. */
  refreshNote: () => void;
}

export interface FileUx {
  openViaDialog: () => Promise<void>;
  openPaths: (paths: string[]) => Promise<void>;
  save: (tab: ScriptTab) => Promise<boolean>;
  saveAs: (tab: ScriptTab) => Promise<boolean>;
  confirmClose: (tab: ScriptTab) => Promise<boolean>;
  confirmQuit: () => Promise<boolean>;
}

export function createFileUx(deps: FileUxDeps): FileUx {
  const { tabs, notify, refreshNote } = deps;

  function adopt(f: OpenedFile) {
    // Never open one file into two tabs: they would race each other's saves and
    // whichever wrote last would silently win.
    const existing = tabs.all().find((t) => t.filePath === f.path);
    if (existing) {
      tabs.activate(existing.id);
      notify(`${f.name} is already open.`);
      return;
    }

    const active = tabs.active();
    const reuseEmpty =
      active && !active.filePath && !tabs.isDirty(active) && tabs.textOf(active).trim() === "";

    if (reuseEmpty && active) {
      tabs.replaceDoc(active, f.contents);
      tabs.markSaved(active, {
        path: f.path,
        name: f.name,
        mtimeMs: f.mtimeMs,
        baseline: tabs.docOf(active),
      });
      active.dialect = f.dialect;
      active.encoding = f.encoding;
      active.lineEnding = f.lineEnding;
    } else {
      tabs.create({
        contents: f.contents,
        title: f.name,
        filePath: f.path,
        dialect: f.dialect,
        encoding: f.encoding,
        lineEnding: f.lineEnding,
        mtimeMs: f.mtimeMs,
      });
    }

    if (f.encoding === "utf-8-lossy") {
      notify(
        `${f.name} is not valid UTF-8. It opened with replacement characters, ` +
          `so saving over it would destroy the original bytes — Save is disabled. ` +
          `Use Save As to write a copy.`,
      );
    } else if (f.large) {
      notify(`${f.name} is large (${(f.sizeBytes / 1048576).toFixed(1)} MB). Linting is eased off.`);
    }
    refreshNote();
  }

  async function openViaDialog() {
    try {
      const f = await api.openFileDialog();
      if (f) adopt(f);
    } catch (err) {
      notify(String(err));
    }
  }

  async function openPaths(paths: string[]) {
    for (const path of paths) {
      try {
        adopt(await api.readFile(path));
      } catch (err) {
        // One bad file in a multi-file drop must not abandon the rest.
        notify(String(err));
      }
    }
  }

  async function writeTo(tab: ScriptTab, path: string, expectMtime: number | null) {
    // Snapshot before the await: anything typed during the write stays unsaved.
    const snapshot = tabs.docOf(tab);
    const text = snapshot.toString();
    const outcome = await api.saveFile(path, text, expectMtime, tab.lineEnding);

    if (outcome.type === "conflict") {
      const pick = await choose(
        "File changed on disk",
        `${tab.title} has been modified since you opened it. Overwriting will discard those changes.`,
        [
          { value: "cancel", label: "Cancel", primary: true },
          { value: "reload", label: "Reload from disk" },
          { value: "overwrite", label: "Overwrite", danger: true },
        ],
      );
      if (pick === "reload") {
        const fresh = await api.readFile(path);
        tabs.replaceDoc(tab, fresh.contents);
        tab.mtimeMs = fresh.mtimeMs;
        tab.encoding = fresh.encoding;
        tab.lineEnding = fresh.lineEnding;
        refreshNote();
        return false;
      }
      if (pick !== "overwrite") return false;
      // Explicit overwrite: drop the expectation and write.
      const forced = await api.saveFile(path, text, null, tab.lineEnding);
      if (forced.type !== "saved") return false;
      tabs.markSaved(tab, {
        path,
        name: path.split("/").pop() ?? tab.title,
        mtimeMs: forced.mtimeMs,
        baseline: snapshot,
      });
      refreshNote();
      return true;
    }

    tabs.markSaved(tab, {
      path,
      name: path.split("/").pop() ?? tab.title,
      mtimeMs: outcome.mtimeMs,
      baseline: snapshot,
    });
    refreshNote();
    return true;
  }

  async function saveAs(tab: ScriptTab): Promise<boolean> {
    const snapshot = tabs.docOf(tab);
    try {
      const saved = await api.saveFileDialog(tab.title, snapshot.toString(), tab.lineEnding);
      if (!saved) return false; // cancelled
      tabs.markSaved(tab, {
        path: saved.path,
        name: saved.name,
        mtimeMs: saved.mtimeMs,
        baseline: snapshot,
      });
      // A copy written by us is valid UTF-8 whatever the source was, so Save
      // becomes available again.
      tab.encoding = "utf-8";
      refreshNote();
      return true;
    } catch (err) {
      notify(String(err));
      return false;
    }
  }

  async function save(tab: ScriptTab): Promise<boolean> {
    if (!tab.filePath) return saveAs(tab);

    if (tab.encoding === "utf-8-lossy") {
      const pick = await choose(
        "Cannot save over this file",
        `${tab.title} was not valid UTF-8, so it was opened with replacement characters. ` +
          `Saving would discard the original bytes. Save a copy instead?`,
        [
          { value: "saveas", label: "Save As…", primary: true },
          { value: "cancel", label: "Cancel" },
        ],
      );
      return pick === "saveas" ? saveAs(tab) : false;
    }

    try {
      return await writeTo(tab, tab.filePath, tab.mtimeMs);
    } catch (err) {
      notify(String(err));
      return false;
    }
  }

  async function confirmClose(tab: ScriptTab): Promise<boolean> {
    if (!tabs.isDirty(tab)) return true;
    const pick = await choose(
      "Unsaved changes",
      `${tab.title} has unsaved changes.`,
      [
        { value: "save", label: "Save", primary: true },
        { value: "cancel", label: "Cancel" },
        { value: "discard", label: "Discard", danger: true },
      ],
    );
    if (pick === "save") return save(tab);
    return pick === "discard";
  }

  async function confirmQuit(): Promise<boolean> {
    const dirty = tabs.all().filter((t) => tabs.isDirty(t));
    if (dirty.length === 0) return true;
    const names = dirty.map((t) => t.title).join(", ");
    const pick = await choose(
      "Unsaved changes",
      dirty.length === 1
        ? `${names} has unsaved changes.`
        : `${dirty.length} tabs have unsaved changes: ${names}.`,
      [
        { value: "cancel", label: "Keep editing", primary: true },
        { value: "discard", label: "Quit anyway", danger: true },
      ],
    );
    return pick === "discard";
  }

  return { openViaDialog, openPaths, save, saveAs, confirmClose, confirmQuit };
}
