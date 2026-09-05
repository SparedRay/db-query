// The connection rail: one icon per connection, click to switch, + to add.
//
// A connection is either *saved* (a profile in connections.json) or *ad hoc*
// (connected once, never written down). Both behave identically while live;
// only ad-hoc ones vanish on quit.
//
// Several connections can be live at once. Switching between them is a change
// of view, not of session — the connection you leave keeps its tabs, its schema
// cache and any query still running on it.

import { api, type ProfileView, type ConnInfo } from "./api";
import { choose } from "./dialog";

export interface ConnectionEntry {
  profile: ProfileView;
  /** Persisted in connections.json, as opposed to a one-off connection. */
  saved: boolean;
  connected: boolean;
  serverVersion: string | null;
  /** Populated on connect; drives the schema tree. */
  databases: string[];
}

export interface ConnectionHooks {
  onActivate: (entry: ConnectionEntry) => void;
  onConnected: (entry: ConnectionEntry, info: ConnInfo) => void;
  /** About to disconnect or delete — return false to abort (unsaved tabs). */
  canDrop?: (entry: ConnectionEntry) => Promise<boolean>;
  onDisconnected: (entry: ConnectionEntry) => void;
  onRemoved: (entry: ConnectionEntry) => void;
  notify: (message: string) => void;
}

let seq = 0;
export const newConnectionId = () => `c${++seq}-${Date.now().toString(36)}`;

export const COLOURS = [
  "#3b82f6", // blue
  "#22c55e", // green
  "#f59e0b", // amber
  "#ef4444", // red — the obvious one for production
  "#a855f7", // violet
  "#14b8a6", // teal
  "#64748b", // slate
];

/** Two letters for the rail disc. Names are chosen by the user, so respect them. */
export function initials(name: string): string {
  const words = name.trim().split(/[\s\-_.]+/).filter(Boolean);
  if (words.length === 0) return "?";
  if (words.length === 1) return words[0].slice(0, 2).toUpperCase();
  return (words[0][0] + words[1][0]).toUpperCase();
}

export class ConnectionManager {
  private entries: ConnectionEntry[] = [];
  private activeId: string | null = null;

  constructor(
    private rail: HTMLElement,
    private hooks: ConnectionHooks,
    private openEditor: (existing?: ConnectionEntry) => void,
  ) {}

  all(): ConnectionEntry[] {
    return this.entries;
  }

  active(): ConnectionEntry | null {
    return this.entries.find((e) => e.profile.id === this.activeId) ?? null;
  }

  get(id: string): ConnectionEntry | undefined {
    return this.entries.find((e) => e.profile.id === id);
  }

  /** Load saved profiles into the rail. They start disconnected. */
  async loadSaved() {
    try {
      const { profiles, warning } = await api.listProfiles();
      if (warning) this.hooks.notify(warning);
      for (const profile of profiles) {
        if (this.get(profile.id)) continue;
        this.entries.push({
          profile,
          saved: true,
          connected: false,
          serverVersion: null,
          databases: [],
        });
      }
      this.render();
    } catch (err) {
      this.hooks.notify(String(err));
    }
  }

  upsert(entry: ConnectionEntry) {
    const i = this.entries.findIndex((e) => e.profile.id === entry.profile.id);
    if (i === -1) this.entries.push(entry);
    else this.entries[i] = entry;
    this.render();
  }

  activate(id: string) {
    const entry = this.get(id);
    if (!entry || id === this.activeId) return;
    this.activeId = id;
    this.render();
    this.hooks.onActivate(entry);
  }

/**
   * Outcome of a connection attempt.
   *
   * The error is *returned*, never shown here, because where it belongs depends
   * on who asked: the rail wants it in the results pane, the editor dialog wants
   * it next to the fields the user is about to correct.
   */
  private async attempt(
    entry: ConnectionEntry,
    open: () => Promise<ConnInfo>,
  ): Promise<{ ok: boolean; error?: string }> {
    try {
      const info = await open();
      entry.connected = true;
      entry.serverVersion = info.serverVersion;
      entry.databases = info.databases;
      // Only now does it join the rail. A failed attempt must leave no trace —
      // otherwise every typo becomes a dead icon the user has to clean up.
      this.upsert(entry);
      this.hooks.onConnected(entry, info);
      this.activate(entry.profile.id);
      return { ok: true };
    } catch (err) {
      return { ok: false, error: String(err) };
    }
  }

  /** Connect with a password the caller has in hand. */
  connectWith(entry: ConnectionEntry, password: string) {
    return this.attempt(entry, () => api.connect(entry.profile, password));
  }

  /** Connect using the password remembered in the keychain. */
  connectStored(entry: ConnectionEntry) {
    return this.attempt(entry, () => api.connectSaved(entry.profile.id));
  }

  /**
   * Clicking a connection in the rail. Uses the remembered password if there is
   * one, otherwise opens the editor to collect it.
   */
  async connect(entry: ConnectionEntry): Promise<{ ok: boolean; error?: string }> {
    if (entry.saved && entry.profile.rememberPassword) {
      const r = await this.connectStored(entry);
      if (!r.ok && r.error) this.hooks.notify(r.error);
      return r;
    }
    this.openEditor(entry);
    return { ok: false };
  }

  async disconnect(entry: ConnectionEntry) {
    if (this.hooks.canDrop && !(await this.hooks.canDrop(entry))) return;
    try {
      await api.disconnect(entry.profile.id);
    } catch (err) {
      this.hooks.notify(String(err));
    }
    entry.connected = false;
    entry.serverVersion = null;
    entry.databases = [];
    this.hooks.onDisconnected(entry);

    // An ad-hoc connection has nothing to go back to, so it leaves the rail.
    if (!entry.saved) {
      this.entries = this.entries.filter((e) => e.profile.id !== entry.profile.id);
      this.hooks.onRemoved(entry);
      if (this.activeId === entry.profile.id) {
        this.activeId = null;
        const next = this.entries.find((e) => e.connected) ?? this.entries[0];
        if (next) this.activate(next.profile.id);
      }
    }
    this.render();
  }

  async remove(entry: ConnectionEntry) {
    if (entry.connected) await this.disconnect(entry);
    if (entry.saved) {
      try {
        // The stored password goes with it — an orphaned secret outlives the
        // thing that explained what it was for.
        const warning = await api.deleteProfile(entry.profile.id);
        if (warning) this.hooks.notify(warning);
      } catch (err) {
        this.hooks.notify(String(err));
      }
    }
    this.entries = this.entries.filter((e) => e.profile.id !== entry.profile.id);
    this.hooks.onRemoved(entry);
    if (this.activeId === entry.profile.id) {
      this.activeId = null;
      const next = this.entries[0];
      if (next) this.activate(next.profile.id);
    }
    this.render();
  }

  render() {
    const nodes = this.entries.map((entry) => {
      const el = document.createElement("button");
      el.className =
        "rail-item" +
        (entry.profile.id === this.activeId ? " active" : "") +
        (entry.connected ? " live" : " offline") +
        (entry.saved ? "" : " adhoc");
      el.style.setProperty("--conn-colour", entry.profile.colour);
      el.textContent = initials(entry.profile.name);
      el.title =
        `${entry.profile.name}\n${entry.profile.user}@${entry.profile.host}:${entry.profile.port}\n` +
        (entry.connected
          ? `Connected — MySQL ${entry.serverVersion ?? "?"}`
          : entry.saved
            ? "Saved, not connected"
            : "Not connected") +
        (entry.saved ? "" : "\n(not saved — this connection is one-off)");

      el.onclick = () => {
        if (entry.connected) this.activate(entry.profile.id);
        else void this.connect(entry);
      };
      el.oncontextmenu = (e) => {
        e.preventDefault();
        this.contextMenu(e, entry);
      };
      return el;
    });

    const add = document.createElement("button");
    add.className = "rail-add";
    add.textContent = "+";
    add.title = "New connection";
    add.onclick = () => this.openEditor();

    this.rail.replaceChildren(...nodes, add);
  }

  private contextMenu(e: MouseEvent, entry: ConnectionEntry) {
    document.querySelector(".ctx-menu")?.remove();
    const menu = document.createElement("div");
    menu.className = "ctx-menu";
    menu.style.left = `${e.clientX}px`;
    menu.style.top = `${e.clientY}px`;

    const items: Array<[string, () => void, boolean?]> = entry.connected
      ? [
          ["Disconnect", () => void this.disconnect(entry)],
          ["Edit…", () => this.openEditor(entry)],
          ["Duplicate", () => this.duplicate(entry)],
          ["Delete", () => void this.confirmRemove(entry), true],
        ]
      : [
          ["Connect", () => void this.connect(entry)],
          ["Edit…", () => this.openEditor(entry)],
          ["Duplicate", () => this.duplicate(entry)],
          ["Delete", () => void this.confirmRemove(entry), true],
        ];

    for (const [label, run, danger] of items) {
      const b = document.createElement("button");
      b.textContent = label;
      if (danger) b.className = "danger";
      b.onclick = () => {
        close();
        run();
      };
      menu.append(b);
    }

    document.body.append(menu);

    // Keep it on screen when opened near an edge — measurable only once
    // attached, which is why this runs after the append.
    const box = menu.getBoundingClientRect();
    if (box.bottom > window.innerHeight) {
      menu.style.top = `${Math.max(4, window.innerHeight - box.height - 4)}px`;
    }
    if (box.right > window.innerWidth) {
      menu.style.left = `${Math.max(4, window.innerWidth - box.width - 4)}px`;
    }

    // Dismiss on a mousedown OUTSIDE the menu. Without the containment check
    // the menu is torn down on mousedown and the button never receives its
    // click, which makes every item silently unclickable.
    const dismiss = (ev: MouseEvent) => {
      if (menu.contains(ev.target as Node)) return;
      close();
    };
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === "Escape") close();
    };
    const close = () => {
      menu.remove();
      document.removeEventListener("mousedown", dismiss, true);
      document.removeEventListener("keydown", onKey);
    };
    document.addEventListener("mousedown", dismiss, true);
    document.addEventListener("keydown", onKey);
  }

  private duplicate(entry: ConnectionEntry) {
    this.openEditor({
      ...entry,
      // A copy is a new connection: new id, no stored password of its own.
      profile: {
        ...entry.profile,
        id: newConnectionId(),
        name: `${entry.profile.name} copy`,
        rememberPassword: false,
      },
      connected: false,
      serverVersion: null,
      databases: [],
    });
  }

  private async confirmRemove(entry: ConnectionEntry) {
    const notes: string[] = [];
    if (entry.saved && entry.profile.rememberPassword) {
      notes.push("Its saved password will also be removed from the system keychain.");
    }
    if (entry.connected) {
      notes.push("It is currently connected; its tabs will be closed.");
    }
    const pick = await choose(
      `Delete "${entry.profile.name}"?`,
      notes.length ? notes.join(" ") : "This cannot be undone.",
      [
        { value: "cancel", label: "Cancel", primary: true },
        { value: "delete", label: "Delete", danger: true },
      ],
    );
    if (pick === "delete") await this.remove(entry);
  }
}
