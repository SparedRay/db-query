/**
 * The schema export dialog.
 *
 * Its own tree rather than checkboxes in the schema sidebar: that pane is the
 * app's main navigation and 260px wide, and this is a bounded task that ends
 * in a decision. See `docs/stage-22-schema-export.md` §3.
 *
 * # The selection means "resolved when it runs"
 *
 * A database ticked whole is sent as `null` for each kind — *everything of
 * that kind, enumerated by the backend inside its snapshot*. Only a partial
 * tick sends names.
 *
 * That is not an optimisation, it is the meaning: the tree lazy-loads, so
 * ticking a database you have never opened must not fetch every table in it,
 * and a table created between the tick and the Export belongs to "the whole
 * database" in any reading somebody would expect.
 */
import { api, type DbPick, type DumpContents, type DumpLayout, type ExportCatalogue } from "./api";

/** The kinds a database's objects are grouped into, in the order they appear. */
const KINDS = ["tables", "views", "routines", "triggers", "events"] as const;
export type Kind = (typeof KINDS)[number];

const LABELS: Record<Kind, string> = {
  tables: "Tables",
  views: "Views",
  routines: "Routines",
  triggers: "Triggers",
  events: "Events",
};

/**
 * What is ticked, per database.
 *
 * `whole` is the database ticked entire — the state that sends `null`. When it
 * is false, `picked` holds the names, per kind.
 */
interface DbState {
  db: string;
  whole: boolean;
  open: boolean;
  catalogue: ExportCatalogue | null;
  loading: boolean;
  picked: Record<Kind, Set<string>>;
}

function emptyPicks(): Record<Kind, Set<string>> {
  return {
    tables: new Set(),
    views: new Set(),
    routines: new Set(),
    triggers: new Set(),
    events: new Set(),
  };
}

function namesOf(catalogue: ExportCatalogue, kind: Kind): string[] {
  switch (kind) {
    case "tables":
      return catalogue.tables.map((t) => t.name);
    case "views":
      return catalogue.views;
    case "routines":
      return catalogue.routines.map((r) => r.name);
    case "triggers":
      return catalogue.triggers.map((t) => t.name);
    case "events":
      return catalogue.events.map((e) => e.name);
  }
}

export interface ExportDeps {
  dialog: HTMLDialogElement;
  tree: HTMLElement;
  filter: HTMLInputElement;
  summary: HTMLElement;
  error: HTMLElement;
  all: HTMLButtonElement;
  none: HTMLButtonElement;
  ok: HTMLButtonElement;
  cancel: HTMLButtonElement;
  batch: HTMLInputElement;
  dropExisting: HTMLInputElement;
  createDatabase: HTMLInputElement;
  createDatabaseNote: HTMLElement;
  omitDefiner: HTMLInputElement;
  /** The connection being exported, and its databases. */
  context: () => { connectionId: string | null; databases: string[]; name: string };
  /** Rows per INSERT, from Settings — the dialog opens on it. */
  defaultBatch: () => number;
  /** Hand the job over. The dialog closes; the run reports elsewhere. */
  start: (picks: DbPick[], contents: DumpContents, layout: DumpLayout, options: {
    dropExisting: boolean;
    createDatabase: boolean;
    omitDefiner: boolean;
    batchRows: number;
  }) => void;
}

export function createExportDialog(deps: ExportDeps) {
  let states: DbState[] = [];

  /** Which kinds a *partially* ticked database contributes, as names. */
  function pickFor(s: DbState): DbPick | null {
    if (s.whole) {
      return { db: s.db, tables: null, views: null, routines: null, triggers: null, events: null };
    }
    const chosen = KINDS.filter((k) => s.picked[k].size > 0);
    if (chosen.length === 0) return null;
    const pick: DbPick = {
      db: s.db,
      tables: [],
      views: [],
      routines: [],
      triggers: [],
      events: [],
    };
    for (const k of KINDS) pick[k] = [...s.picked[k]];
    return pick;
  }

  function picks(): DbPick[] {
    return states.map(pickFor).filter((p): p is DbPick => p !== null);
  }

  /** Whole, partly, or not at all — the three a parent checkbox can be in. */
  function stateOf(s: DbState): "all" | "some" | "none" {
    if (s.whole) return "all";
    const n = KINDS.reduce((sum, k) => sum + s.picked[k].size, 0);
    if (n === 0) return "none";
    // Everything individually ticked still reads as "some": it was chosen
    // object by object, and a table added later is not part of that choice.
    return "some";
  }

  function contents(): DumpContents {
    const el = deps.dialog.querySelector<HTMLInputElement>('input[name="exp-contents"]:checked');
    return (el?.value as DumpContents) ?? "schemaAndData";
  }

  function layout(): DumpLayout {
    const el = deps.dialog.querySelector<HTMLInputElement>('input[name="exp-layout"]:checked');
    return (el?.value as DumpLayout) ?? "singleFile";
  }

  // ------------------------------------------------------------- drawing

  function row(opts: {
    label: string;
    state: "all" | "some" | "none";
    count?: string;
    group?: boolean;
    twisty?: "open" | "closed" | null;
    onToggleOpen?: () => void;
    onCheck: (on: boolean) => void;
  }): HTMLElement {
    const el = document.createElement("div");
    el.className = `exp-node${opts.group ? " group" : ""}`;

    const twisty = document.createElement("span");
    twisty.className = "twisty";
    if (opts.twisty) {
      twisty.textContent = opts.twisty === "open" ? "▾" : "▸";
      twisty.onclick = opts.onToggleOpen ?? null;
    }

    const box = document.createElement("input");
    box.type = "checkbox";
    box.checked = opts.state === "all";
    // The third state. Not a class: `indeterminate` is what a screen reader
    // reads as "mixed", and it is what the browser draws as a dash.
    box.indeterminate = opts.state === "some";
    box.onchange = () => opts.onCheck(box.checked);

    const name = document.createElement("span");
    name.className = "exp-name";
    name.textContent = opts.label;

    el.append(twisty, box, name);
    if (opts.count) {
      const c = document.createElement("span");
      c.className = "exp-count";
      c.textContent = opts.count;
      el.append(c);
    }
    return el;
  }

  function matches(name: string): boolean {
    const q = deps.filter.value.trim().toLowerCase();
    return q === "" || name.toLowerCase().includes(q);
  }

  function draw() {
    deps.tree.replaceChildren();
    for (const s of states) {
      const wrap = document.createElement("div");
      wrap.append(
        row({
          label: s.db,
          state: stateOf(s),
          count: s.catalogue ? `${s.catalogue.tables.length} tables` : "",
          twisty: s.open ? "open" : "closed",
          onToggleOpen: () => void toggleOpen(s),
          onCheck: (on) => {
            s.whole = on;
            if (!on) s.picked = emptyPicks();
            draw();
          },
        }),
      );

      if (s.open) {
        const kids = document.createElement("div");
        kids.className = "exp-kids";
        if (s.loading) {
          const note = document.createElement("div");
          note.className = "exp-node group";
          note.textContent = "Reading…";
          kids.append(note);
        } else if (s.catalogue) {
          for (const kind of KINDS) {
            const names = namesOf(s.catalogue, kind).filter(matches);
            if (names.length === 0) continue;
            const chosen = s.picked[kind];
            const groupState: "all" | "some" | "none" = s.whole
              ? "all"
              : chosen.size === 0
                ? "none"
                : chosen.size === names.length
                  ? "all"
                  : "some";
            kids.append(
              row({
                label: LABELS[kind],
                state: groupState,
                count: String(names.length),
                group: true,
                twisty: null,
                onCheck: (on) => {
                  // Ticking a group of a whole database has to materialise the
                  // other kinds first, or unticking one kind would silently
                  // drop the rest.
                  if (s.whole) materialise(s);
                  if (on) names.forEach((n) => s.picked[kind].add(n));
                  else names.forEach((n) => s.picked[kind].delete(n));
                  draw();
                },
              }),
            );
            for (const name of names) {
              const on = s.whole || chosen.has(name);
              const extra =
                kind === "tables"
                  ? s.catalogue.tables.find((t) => t.name === name)?.estimatedRows
                  : undefined;
              const kid = row({
                label: name,
                state: on ? "all" : "none",
                count: extra !== undefined ? `~${extra.toLocaleString()}` : "",
                twisty: null,
                onCheck: (checked) => {
                  if (s.whole) materialise(s);
                  if (checked) s.picked[kind].add(name);
                  else s.picked[kind].delete(name);
                  draw();
                },
              });
              kid.style.marginLeft = "16px";
              kids.append(kid);
            }
          }
        }
        wrap.append(kids);
      }
      deps.tree.append(wrap);
    }
    summarise();
  }

  /**
   * Turn "the whole database" into the names it currently stands for.
   *
   * Only when somebody unticks part of it: until then `whole` is the more
   * useful state, because it keeps meaning "all of it" as the database
   * changes.
   */
  function materialise(s: DbState) {
    if (!s.whole || !s.catalogue) return;
    s.whole = false;
    for (const kind of KINDS) {
      s.picked[kind] = new Set(namesOf(s.catalogue, kind));
    }
  }

  async function toggleOpen(s: DbState) {
    s.open = !s.open;
    if (s.open && !s.catalogue && !s.loading) {
      const { connectionId } = deps.context();
      if (connectionId) {
        s.loading = true;
        draw();
        try {
          s.catalogue = await api.exportCatalogue(connectionId, s.db);
        } catch (err) {
          deps.error.hidden = false;
          deps.error.textContent = String(err);
        } finally {
          s.loading = false;
        }
      }
    }
    draw();
  }

  function summarise() {
    const chosen = picks();
    if (chosen.length === 0) {
      deps.summary.textContent = "Nothing selected.";
      deps.ok.disabled = true;
      return;
    }
    deps.ok.disabled = false;

    let tables = 0;
    let rows = 0;
    let unknown = false;
    for (const pick of chosen) {
      const s = states.find((x) => x.db === pick.db)!;
      if (!s.catalogue) {
        // Ticked without ever being opened, which is the common case and the
        // whole point of resolving later. Counting it would mean fetching it.
        unknown = true;
        continue;
      }
      const names = pick.tables ?? namesOf(s.catalogue, "tables");
      tables += names.length;
      for (const t of s.catalogue.tables) {
        if (names.includes(t.name)) rows += t.estimatedRows;
      }
    }

    const parts = [`${chosen.length} database${chosen.length === 1 ? "" : "s"}`];
    if (tables) parts.push(`${tables} table${tables === 1 ? "" : "s"}`);
    if (rows && contents() !== "schemaOnly") {
      // `~` because it is `information_schema`'s estimate, which is about 1%
      // out on a million rows and is never used for anything but this line.
      parts.push(`~${rows.toLocaleString()} rows`);
    }
    if (unknown) parts.push("plus databases not yet opened");
    deps.summary.textContent = parts.join(" · ");
  }

  // ------------------------------------------------------------- wiring

  deps.filter.oninput = () => draw();
  deps.all.onclick = () => {
    states.forEach((s) => {
      s.whole = true;
    });
    draw();
  };
  deps.none.onclick = () => {
    states.forEach((s) => {
      s.whole = false;
      s.picked = emptyPicks();
    });
    draw();
  };
  deps.cancel.onclick = () => deps.dialog.close();
  for (const el of deps.dialog.querySelectorAll('input[name="exp-contents"]')) {
    (el as HTMLInputElement).onchange = () => {
      summarise();
      syncNotes();
    };
  }
  for (const el of deps.dialog.querySelectorAll('input[name="exp-layout"]')) {
    (el as HTMLInputElement).onchange = () => syncNotes();
  }
  deps.createDatabase.onchange = () => syncNotes();

  /** What the two options that change the script's shape actually mean. */
  function syncNotes() {
    deps.createDatabaseNote.textContent = deps.createDatabase.checked
      ? "The script recreates each database by name and switches into it."
      : "The script names no database, so it loads into whichever one you " +
        "point it at — which is how you restore a copy under a different name.";
  }

  deps.ok.onclick = () => {
    const chosen = picks();
    if (chosen.length === 0) return;
    deps.error.hidden = true;
    deps.dialog.close();
    deps.start(chosen, contents(), layout(), {
      dropExisting: deps.dropExisting.checked,
      createDatabase: deps.createDatabase.checked,
      omitDefiner: deps.omitDefiner.checked,
      batchRows: Math.max(1, Math.min(10_000, Number(deps.batch.value) || deps.defaultBatch())),
    });
  };

  return {
    /** Open it, optionally with one database already ticked. */
    open(preselect?: string | null) {
      const { databases } = deps.context();
      states = databases.map((db) => ({
        db,
        whole: preselect === db,
        open: preselect === db,
        catalogue: null,
        loading: false,
        picked: emptyPicks(),
      }));
      deps.filter.value = "";
      deps.error.hidden = true;
      deps.batch.value = String(deps.defaultBatch());
      syncNotes();
      draw();
      deps.dialog.showModal();
      // After the dialog is up, so the fetch does not delay it appearing.
      const opened = states.find((s) => s.open);
      if (opened) void loadInto(opened);
    },
  };

  async function loadInto(s: DbState) {
    const { connectionId } = deps.context();
    if (!connectionId || s.catalogue || s.loading) return;
    s.loading = true;
    draw();
    try {
      s.catalogue = await api.exportCatalogue(connectionId, s.db);
    } catch (err) {
      deps.error.hidden = false;
      deps.error.textContent = String(err);
    } finally {
      s.loading = false;
      draw();
    }
  }
}
