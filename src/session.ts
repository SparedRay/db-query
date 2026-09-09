// The open tabs, remembered across restarts.
//
// # Restore happens on connect, not at boot
//
// A tab belongs to a connection for life, and nothing connects at boot — that
// is a network action against someone else's server, and it would prompt for
// passwords the keychain does not have. So a workspace waits, fully loaded, in
// `pending` until its connection comes up, and materialises then.
//
// # What is stored
//
// A **clean file-backed tab stores only its path** and is re-read on restore,
// which keeps the ordinary case to a few hundred bytes. Text is written only
// when it is the only copy in existence: an untitled buffer, or a file-backed
// tab with unsaved edits.
//
// Nothing describing a *result* is stored. Results are unbounded and stale by
// definition, and painting yesterday's rows as though they were a result is the
// kind of wrong that costs someone a decision.

import { api, type SessionStore, type StoredTab, type StoredWorkspace } from "./api";
import type { RestoredTab, TabManager } from "./tabs";

/** How long the buffer may run ahead of the file. A crash costs at most this. */
const DEBOUNCE_MS = 1000;

export interface SessionPersistence {
  /** Read the file. Returns a warning if it was unreadable and moved aside. */
  boot(): Promise<string | null>;
  /**
   * Materialise a connection's stored tabs, if it has any and has none live.
   *
   * Warnings are **returned rather than shown**: this runs inside `onConnected`,
   * and whatever it wrote to the results pane would be overwritten by the
   * "Connected to …" line a moment later. The caller says everything at once.
   */
  restoreInto(connectionId: string): Promise<{ restored: number; warnings: string[] }>;
  /** Remember the current state soon. Cheap; call it freely. */
  schedule(): void;
  /** Write now and wait for it — for quitting. */
  flush(): Promise<void>;
  /** A connection was removed: its stored tabs go with it. */
  forget(connectionId: string): void;
}

export function createSessionPersistence(deps: {
  tabs: () => TabManager;
  notify: (message: string) => void;
}): SessionPersistence {
  /**
   * Workspaces read from disk that have not been materialised yet — because
   * their connection has not come up this session, and may never.
   *
   * They must be written back out untouched on every save, or connecting to one
   * server would quietly delete the remembered tabs of every other.
   */
  let pending = new Map<string, StoredWorkspace>();
  let timer: ReturnType<typeof setTimeout> | null = null;
  /**
   * The read, so everything else can wait for it.
   *
   * Nothing may be written before it resolves, or the first keystroke would
   * save an empty session over the remembered one — and nothing may be
   * *restored* before it either, since connecting fast enough to beat the read
   * would otherwise leave the tabs on disk and an empty workspace on screen.
   */
  let loaded: Promise<void> | null = null;

  function snapshot(): SessionStore {
    const tabs = deps.tabs();
    const byConnection = new Map<string, StoredWorkspace>();

    // Live tabs first, in tab-strip order.
    for (const tab of tabs.all()) {
      let ws = byConnection.get(tab.connectionId);
      if (!ws) {
        ws = { connectionId: tab.connectionId, tabs: [], activeIndex: 0 };
        byConnection.set(tab.connectionId, ws);
      }
      const dirty = tabs.isDirty(tab);
      ws.tabs.push({
        title: tab.title,
        filePath: tab.filePath,
        dialect: tab.dialect,
        encoding: tab.encoding,
        lineEnding: tab.lineEnding,
        mtimeMs: tab.mtimeMs,
        // The only copy, or nothing at all.
        text: tab.filePath === null || dirty ? tabs.textOf(tab) : null,
        cursor: tabs.cursorOf(tab),
        activeDb: tab.activeDb,
        untitledNumber: tab.untitledNumber,
      });
    }

    for (const [connectionId, ws] of byConnection) {
      const front = tabs.frontOf(connectionId);
      const at = tabs.forConnection(connectionId).findIndex((t) => t.id === front?.id);
      ws.activeIndex = at === -1 ? 0 : at;
    }

    // Then everything we have not touched this session, exactly as it was read.
    //
    // **Untouched is the invariant.** Stage 12 tried to prune workspaces whose
    // connection is not a saved profile, to stop the file growing by one entry
    // per deleted profile and per ad-hoc connection. Two tests said no: an id
    // we do not recognise is not the same as an id that is gone — it is a
    // profile this build has not read yet, or one whose connection is about to
    // come up — and deleting on that basis is exactly the "connecting to one
    // server forgets another's tabs" bug these tests exist to prevent. A
    // profile removed through the UI is handled by `forget`, where the fact is
    // actually known.
    for (const [connectionId, ws] of pending) {
      if (!byConnection.has(connectionId)) byConnection.set(connectionId, ws);
    }

    return { version: 1, connections: [...byConnection.values()] };
  }

  async function write() {
    if (!loaded) return;
    await loaded;
    try {
      await api.saveSession(snapshot());
    } catch (err) {
      // Failing to remember tabs must never interrupt what someone is doing.
      // It is reported once, through the same channel as everything else.
      deps.notify(`Could not remember the open tabs: ${String(err)}`);
    }
  }

  /**
   * Turn one stored tab into something `TabManager.restore` can build, reading
   * the file where there is one.
   *
   * Returns null when the tab should not come back at all — which is only ever
   * the case when it held nothing that is not already on disk.
   */
  async function resolve(
    stored: StoredTab,
    warn: (message: string) => void,
  ): Promise<RestoredTab | null> {
    const base: RestoredTab = {
      title: stored.title,
      filePath: null,
      dialect: stored.dialect || "mysql",
      encoding: stored.encoding || "utf-8",
      lineEnding: stored.lineEnding || "lf",
      mtimeMs: null,
      contents: stored.text ?? "",
      baseline: stored.text ?? "",
      cursor: stored.cursor,
      activeDb: stored.activeDb,
      untitledNumber: stored.untitledNumber,
    };

    if (stored.filePath === null) return base;

    let file;
    try {
      file = await api.readFile(stored.filePath);
    } catch {
      // The file went away while the app was closed.
      if (stored.text === null) {
        // A clean tab held nothing the file did not. Dropping it loses no work.
        warn(`${stored.title} was not restored — ${stored.filePath} is no longer there.`);
        return null;
      }
      // Unsaved edits, and nowhere to put them back. Keep the buffer and let go
      // of the path, so Save asks where it should go rather than failing.
      warn(`${stored.title} kept its unsaved changes, but ${stored.filePath} is gone.`);
      return base;
    }

    if (stored.text === null) {
      // Clean: it *is* the file, so it takes the file's current mtime and there
      // is nothing to conflict with.
      return {
        ...base,
        filePath: file.path,
        dialect: file.dialect,
        encoding: file.encoding,
        lineEnding: file.lineEnding,
        mtimeMs: file.mtimeMs,
        contents: file.contents,
        baseline: file.contents,
      };
    }

    // Dirty: the buffer is ours, the baseline is whatever is on disk now — and
    // it keeps the mtime it was **based on**, so if the file moved on while the
    // app was closed the first Save asks before overwriting it. That is the
    // existing conflict check, reused rather than a second one invented.
    return {
      ...base,
      filePath: file.path,
      dialect: file.dialect,
      encoding: file.encoding,
      lineEnding: file.lineEnding,
      mtimeMs: stored.mtimeMs,
      contents: stored.text,
      baseline: file.contents,
    };
  }

  return {
    boot() {
      let warning: string | null = null;
      loaded = (async () => {
        try {
          const out = await api.loadSession();
          pending = new Map(out.session.connections.map((w) => [w.connectionId, w]));
          warning = out.warning;
        } catch (err) {
          // Start with no memory rather than not starting.
          warning = `Could not read the saved session: ${String(err)}`;
        }
      })();
      return loaded.then(() => warning);
    },

    async restoreInto(connectionId) {
      // Connecting can easily beat the file read at boot.
      if (loaded) await loaded;
      const stored = pending.get(connectionId);
      if (!stored) return { restored: 0, warnings: [] };
      // Taken before the first await: a second connect while this one is in
      // flight must not restore the same tabs twice.
      pending.delete(connectionId);

      const tabs = deps.tabs();
      // Reconnecting inside one session — the live tabs are the truth.
      if (tabs.forConnection(connectionId).length > 0) return { restored: 0, warnings: [] };

      const warnings: string[] = [];
      const specs: RestoredTab[] = [];
      for (const t of stored.tabs) {
        const spec = await resolve(t, (m) => warnings.push(m));
        if (spec) specs.push(spec);
      }
      if (specs.length > 0) tabs.restore(connectionId, specs, stored.activeIndex);
      return { restored: specs.length, warnings };
    },

    schedule() {
      if (timer !== null) clearTimeout(timer);
      timer = setTimeout(() => {
        timer = null;
        void write();
      }, DEBOUNCE_MS);
    },

    async flush() {
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
      await write();
    },

    forget(connectionId) {
      pending.delete(connectionId);
      this.schedule();
    },
  };
}
