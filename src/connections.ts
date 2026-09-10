// The connection rail: one icon per connection, click to switch, + to add.
//
// A connection is either *saved* (a profile in connections.json) or *ad hoc*
// (connected once, never written down). Both behave identically while live;
// only ad-hoc ones vanish on quit.
//
// Several connections can be live at once. Switching between them is a change
// of view, not of session — the connection you leave keeps its tabs, its schema
// cache and any query still running on it.

import { api, type Capabilities, type ProfileView, type ConnInfo } from "./api";
import { contextMenu } from "./menu";
import { choose } from "./dialog";

export interface ConnectionEntry {
  profile: ProfileView;
  /** Persisted in connections.json, as opposed to a one-off connection. */
  saved: boolean;
  connected: boolean;
  /** An attempt is in flight. Drives the rail spinner; never persisted. */
  connecting?: boolean;
  serverVersion: string | null;
  /**
   * What this connection's engine supports. Null until it has connected once —
   * capabilities come from the server, not from the saved profile, so a profile
   * copied from another machine cannot claim the wrong ones.
   */
  capabilities: Capabilities | null;
  /** Populated on connect; drives the schema tree. */
  databases: string[];
}

export interface ConnectionHooks {
  onActivate: (entry: ConnectionEntry) => void;
  /**
   * Awaited before the connection becomes the visible workspace, because
   * restoring its remembered tabs has to finish first — `setActiveConnection`
   * creates an empty tab for a workspace it finds empty, and would race it.
   */
  onConnected: (entry: ConnectionEntry, info: ConnInfo) => void | Promise<void>;
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

/**
 * How to name this connection's engine in the UI.
 *
 * "MySQL" was hardcoded when there was only one engine. It comes from the
 * server now, so a cluster does not describe itself as MySQL — and an entry
 * that has never connected has no engine to name yet.
 */
/**
 * Where this connection points, in the terms its engine actually uses.
 *
 * MySQL is addressed by host and port, Elasticsearch by a URL. The editor
 * hides the fields the chosen engine does not use, but it does not blank
 * them — so a saved cluster still carries whatever the MySQL half defaulted
 * to, and reading `host:port` off one reported `@localhost:3306` for a server
 * that was never at either.
 *
 * Branches on `kind` rather than on capabilities, deliberately: capabilities
 * arrive with the first successful connection, and this has to be right on a
 * saved profile that has never connected — which is exactly when someone
 * hovers a rail button to find out what it is.
 */
export function address(p: ProfileView): string {
  if (p.kind === "elasticsearch") {
    const user = p.auth?.type === "basic" ? p.auth.user : "";
    const url = p.url || "(no URL)";
    return user ? `${user}@${url}` : url;
  }
  return `${p.user}@${p.host}:${p.port}`;
}

function engineLabel(entry: ConnectionEntry): string {
  const engine = entry.capabilities?.engine;
  if (!engine) return "Server";
  return engine === "mysql" ? "MySQL" : engine;
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
          capabilities: null,
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
    // Opening a connection is the slowest thing this app does — DNS, TCP, TLS,
    // then MySQL's own handshake. Clicking a saved connection used to give no
    // sign of life until it either appeared or failed, which reads as a broken
    // button rather than as work in progress.
    entry.connecting = true;
    this.render();
    try {
      const info = await open();
      entry.connected = true;
      entry.serverVersion = info.serverVersion;
      entry.capabilities = info.capabilities;
      entry.databases = info.databases;
      // Only now does it join the rail. A failed attempt must leave no trace —
      // otherwise every typo becomes a dead icon the user has to clean up.
      this.upsert(entry);
      await this.hooks.onConnected(entry, info);
      this.activate(entry.profile.id);
      return { ok: true };
    } catch (err) {
      return { ok: false, error: String(err) };
    } finally {
      entry.connecting = false;
      this.render();
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
    // The question is "do we have what it takes", and there are two separate
    // ways the answer is yes: a secret is stored, or none is needed at all.
    //
    // Only the first was ever asked, which is why a cluster with **no
    // authentication** — nothing to remember, so nothing remembered — opened
    // the connect dialog on every click, forever.
    //
    // `=== false`, not `!`: an absent `needsSecret` means "we do not know", and
    // not knowing must fall back to asking. Keeping `rememberPassword` as an
    // independent reason matters for the same case in reverse — were
    // `needsSecret` ever missing or wrong, gating on it alone would prompt for
    // every remembered password, which is precisely the Stage 2 bug two tests
    // in `regressions.spec.ts` exist to prevent.
    const ready = entry.profile.rememberPassword || entry.profile.needsSecret === false;
    if (entry.saved && ready) {
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
    entry.capabilities = null;
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
        (entry.connecting ? " connecting" : "") +
        (entry.saved ? "" : " adhoc");
      el.style.setProperty("--conn-colour", entry.profile.colour);
      el.textContent = initials(entry.profile.name);
      el.title =
        `${entry.profile.name}\n${address(entry.profile)}\n` +
        (entry.connected
          ? `Connected — ${engineLabel(entry)} ${entry.serverVersion ?? "?"}`
          : entry.saved
            ? "Saved, not connected"
            : "Not connected") +
        (entry.saved ? "" : "\n(not saved — this connection is one-off)");

      if (entry.connecting) {
        el.setAttribute("aria-busy", "true");
        el.append(Object.assign(document.createElement("span"), { className: "spinner" }));
      }

      el.onclick = () => {
        // A second click while the first is still in flight would open a
        // second connection to the same server.
        if (entry.connecting) return;
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
    const at = this.entries.findIndex((x) => x.profile.id === entry.profile.id);
    contextMenu(e, [
      entry.connected
        ? { label: "Disconnect", run: () => void this.disconnect(entry) }
        : { label: "Connect", run: () => void this.connect(entry) },
      { label: "Edit…", run: () => this.openEditor(entry) },
      { label: "Duplicate", run: () => this.duplicate(entry) },
      // Disabled rather than absent at the ends: a menu whose items move around
      // is one you have to read every time.
      { label: "Move up", run: () => void this.move(entry, -1), disabled: at <= 0 },
      {
        label: "Move down",
        run: () => void this.move(entry, 1),
        disabled: at < 0 || at >= this.entries.length - 1,
      },
      { label: "Delete", run: () => void this.confirmRemove(entry), danger: true },
    ]);
  }

  /**
   * Move a connection one place along the rail, and remember where it landed.
   *
   * Two menu items rather than a drag, on purpose: a drag needs drop-position
   * feedback and behaves differently under each of the two webviews this app
   * ships on, and the job here is "put the one I use most at the top".
   *
   * The order is written to disk immediately. A reorder that has to be redone
   * after every restart is not a feature, it is a chore.
   */
  private async move(entry: ConnectionEntry, delta: -1 | 1) {
    const at = this.entries.findIndex((x) => x.profile.id === entry.profile.id);
    const to = at + delta;
    if (at < 0 || to < 0 || to >= this.entries.length) return;
    [this.entries[at], this.entries[to]] = [this.entries[to], this.entries[at]];
    this.render();
    await this.persistOrder();
  }

  /**
   * The rail's order, saved.
   *
   * Only the saved ones are named: an ad-hoc connection has no file entry to
   * order, and it still moves in the rail for as long as it exists. Rust keeps
   * anything the list does not mention, so naming a subset cannot delete
   * anything.
   */
  private async persistOrder() {
    const ids = this.entries.filter((x) => x.saved).map((x) => x.profile.id);
    try {
      await api.reorderProfiles(ids);
    } catch (err) {
      this.hooks.notify(String(err));
    }
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
      capabilities: null,
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
