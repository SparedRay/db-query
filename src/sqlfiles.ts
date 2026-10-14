// The "run SQL files from a folder" dialog.
//
// It owns the **choice**: which connection and database, which folder, where
// the CSVs go, and how they are written. It does not run anything — the run
// reports into the job strip and the report band, outside this dialog, so the
// app stays usable while a folder of scripts takes minutes.
//
// The preview is the feature. "Execute every script in this folder" on a folder
// somebody else assembled is a loaded gun, so what was found is shown before
// Run can be pressed, and the writes are counted. The confirmation that names
// them lives in `main.ts` next to the run, because it is the last thing between
// the choice and the work.

import { api, type CsvOptions, type SqlFileScanned, type SqlFilesScan } from "./api";

export interface SqlFilesDeps {
  dialog: HTMLDialogElement;
  connection: HTMLSelectElement;
  database: HTMLSelectElement;
  pickSource: HTMLButtonElement;
  source: HTMLElement;
  recursive: HTMLInputElement;
  preview: HTMLElement;
  summary: HTMLElement;
  pickDest: HTMLButtonElement;
  dest: HTMLElement;
  error: HTMLElement;
  ok: HTMLButtonElement;
  cancel: HTMLButtonElement;
  csv: {
    delimiter: HTMLInputElement;
    headers: HTMLInputElement;
    nullAs: HTMLInputElement;
    crlf: HTMLInputElement;
    bom: HTMLInputElement;
    guard: HTMLInputElement;
  };
  /** Connections to offer, and their databases. */
  context: () => {
    connections: { id: string; name: string; databases: string[]; canRun: boolean }[];
    activeConnectionId: string | null;
    currentDatabase: (connectionId: string) => string | null;
  };
  /** Why Run is unavailable even with a valid choice — a job already running. */
  blockedBecause: () => string | null;
  start: (plan: SqlFilesPlan) => void;
}

export interface SqlFilesPlan {
  connectionId: string;
  database: string | null;
  source: string;
  dest: string;
  scan: SqlFilesScan;
  csv: CsvOptions;
}

export interface SqlFilesDialog {
  open: (preselect: { connectionId?: string | null; database?: string | null }) => void;
}

/** One line per file in the preview, with the four shapes a file can have. */
function fileRow(f: SqlFileScanned): HTMLElement {
  const row = document.createElement("div");
  row.className = "sf-file";
  row.dataset.path = f.path;

  const mark = document.createElement("span");
  mark.className = "sf-mark";
  const name = document.createElement("span");
  name.className = "sf-name";
  name.textContent = f.path;
  const note = document.createElement("span");
  note.className = "sf-note";

  if (f.verdict.type === "ready") {
    const outputs = f.statements.filter((s) => s.output !== null).length;
    const writes = f.statements.filter((s) => s.writes).length;
    mark.textContent = "•";
    // What it will do, in the order somebody cares: how many statements, how
    // many CSVs, and — last and loudest — whether any of them write.
    const parts = [`${f.statements.length} statement${f.statements.length === 1 ? "" : "s"}`];
    if (outputs > 0) parts.push(`${outputs} CSV${outputs === 1 ? "" : "s"}`);
    if (writes > 0) parts.push(`${writes} write${writes === 1 ? "" : "s"}`);
    note.textContent = parts.join(" · ");
    if (writes > 0) row.classList.add("writes");
  } else {
    // Not an error: a file that will not run is a fact, and showing it is how
    // somebody learns their DELIMITER script needs an editor tab instead.
    mark.textContent = "–";
    row.classList.add("skipped");
    note.textContent =
      f.verdict.type === "nothingToRun"
        ? "Nothing to run — comments only."
        : f.verdict.type === "cannotSplit"
          ? "DELIMITER block — cannot be split into statements."
          : f.verdict.reason;
  }

  row.append(mark, name, note);
  return row;
}

export function createSqlFilesDialog(deps: SqlFilesDeps): SqlFilesDialog {
  let source: string | null = null;
  let dest: string | null = null;
  let scan: SqlFilesScan | null = null;
  let scanning = false;

  const csvOptions = (): CsvOptions => ({
    delimiter: deps.csv.delimiter.value || ",",
    crlf: deps.csv.crlf.checked,
    bom: deps.csv.bom.checked,
    headers: deps.csv.headers.checked,
    nullAs: deps.csv.nullAs.value,
    formulaGuard: deps.csv.guard.checked,
  });

  const say = (msg: string | null) => {
    deps.error.textContent = msg ?? "";
    deps.error.hidden = msg === null;
  };

  /**
   * Why Run is unavailable, and whether that is news or just the next step.
   *
   * **One place.** Reported in use: a folder with no `.sql` files disabled Run
   * and explained itself only in the grey hint line, because that branch
   * returned early and never reached the `say()` its sibling case used. The
   * press then did nothing with nothing to read. Five branches each
   * remembering to speak is five chances to forget; this is one.
   *
   * `error` is for "we looked, and there is nothing here" — something the
   * person needs to act on. `hint` is for "this is the next step", which is
   * not a failure and should not be red.
   */
  function blocker(): { why: string; as: "error" | "hint" } | null {
    // **The dialog opens even with nothing to run, and says why** — Stage 20
    // §11.1's rule: opening is how you read why it is not ready, so taking the
    // click away hides the explanation behind the thing needing explanation.
    // These two live here rather than in `open()`, where they used to be said
    // once and then painted over by the first redraw.
    const { connections } = deps.context();
    if (connections.length === 0) {
      return {
        why: "No connection is open. Connect first, then run a folder against it.",
        as: "error",
      };
    }
    if (!deps.connection.value) {
      return {
        why: "None of the open connections can run a folder of SQL scripts.",
        as: "error",
      };
    }
    if (!source) return { why: "Choose a folder of .sql files.", as: "hint" };
    if (!scan) return { why: "Reading the folder…", as: "hint" };
    if (scan.files.length === 0) {
      return { why: "No .sql files in that folder.", as: "error" };
    }
    if (scan.runnableFiles === 0) {
      return { why: "Nothing in that folder can be run.", as: "error" };
    }
    if (!dest) return { why: "Choose a destination folder for the CSVs.", as: "hint" };
    const busy = deps.blockedBecause();
    return busy === null ? null : { why: busy, as: "error" };
  }

  function drawPreview() {
    deps.preview.replaceChildren();
    if (scanning) {
      deps.summary.textContent = "Reading the folder…";
      deps.ok.disabled = true;
      deps.ok.title = "";
      say(null);
      return;
    }

    for (const f of scan?.files ?? []) deps.preview.append(fileRow(f));

    const blocked = blocker();
    // **Disabled with the reason on it**, always — a disabled button that
    // explains nothing is the bug this came from.
    deps.ok.disabled = blocked !== null;
    deps.ok.title = blocked?.why ?? "";

    // The counts, when there are any files to count.
    if (scan && scan.files.length > 0) {
      const bits = [
        `${scan.runnableFiles} of ${scan.files.length} file${scan.files.length === 1 ? "" : "s"}`,
        `${scan.statements} statement${scan.statements === 1 ? "" : "s"}`,
        `${scan.outputs} CSV${scan.outputs === 1 ? "" : "s"}`,
      ];
      // Last, so it is the thing the sentence ends on, and only when there are
      // any — "0 write" reads as reassurance nobody asked for.
      if (scan.writes > 0) {
        bits.push(`${scan.writes} statement${scan.writes === 1 ? "" : "s"} will write`);
      }
      deps.summary.textContent = bits.join(" · ");
    } else {
      deps.summary.textContent = "";
    }

    if (blocked?.as === "error") {
      say(blocked.why);
      return;
    }
    say(null);
    if (blocked) {
      // A step rather than a failure: it belongs beside the counts, not in red.
      deps.summary.textContent = deps.summary.textContent
        ? `${deps.summary.textContent} · ${blocked.why.toLowerCase()}`
        : blocked.why;
      return;
    }

    // **Picking the source as the destination is allowed, and said.** §3.2
    // chose two folders so inputs never mix with outputs; somebody who points
    // them at the same place has undone that, and should know rather than find
    // CSVs among their scripts later. Not refused — it is their folder.
    if (dest && dest === source) {
      say("The CSVs will be written among the scripts. Choose another folder to keep them apart.");
    }
  }

  async function rescan() {
    const connectionId = deps.connection.value;
    if (!source || !connectionId) {
      drawPreview();
      return;
    }
    scanning = true;
    drawPreview();
    try {
      scan = await api.sqlfilesScan(connectionId, source, deps.recursive.checked);
      say(null);
    } catch (err) {
      scan = null;
      say(String(err));
    } finally {
      scanning = false;
      drawPreview();
    }
  }

  function fillDatabases() {
    const { connections, currentDatabase } = deps.context();
    const entry = connections.find((c) => c.id === deps.connection.value);
    const current = currentDatabase(deps.connection.value);
    deps.database.replaceChildren();
    // **"Whatever the connection is on" is an option, not the absence of one.**
    // A folder of fully-qualified scripts needs no database, and forcing a
    // choice there would be asking about something that does not matter.
    const any = document.createElement("option");
    any.value = "";
    any.textContent = "(leave as it is)";
    deps.database.append(any);
    for (const db of entry?.databases ?? []) {
      const o = document.createElement("option");
      o.value = db;
      o.textContent = db;
      deps.database.append(o);
    }
    deps.database.value = current && (entry?.databases ?? []).includes(current) ? current : "";
  }

  deps.connection.onchange = () => {
    fillDatabases();
    void rescan();
  };
  deps.recursive.onchange = () => void rescan();

  deps.pickSource.onclick = async () => {
    const picked = await api.sqlfilesPickFolder();
    if (!picked) return;
    source = picked;
    deps.source.textContent = picked;
    deps.source.title = picked;
    await rescan();
  };

  deps.pickDest.onclick = async () => {
    const picked = await api.sqlfilesPickFolder();
    if (!picked) return;
    dest = picked;
    deps.dest.textContent = picked;
    deps.dest.title = picked;
    drawPreview();
  };

  deps.cancel.onclick = () => deps.dialog.close();
  deps.dialog.addEventListener("cancel", (e) => {
    e.preventDefault();
    deps.dialog.close();
  });

  deps.ok.onclick = () => {
    if (!scan || !source || !dest) return;
    const plan: SqlFilesPlan = {
      connectionId: deps.connection.value,
      database: deps.database.value || null,
      source,
      dest,
      scan,
      csv: csvOptions(),
    };
    deps.dialog.close();
    deps.start(plan);
  };

  return {
    open(preselect) {
      const { connections, activeConnectionId } = deps.context();
      deps.connection.replaceChildren();
      for (const c of connections) {
        const o = document.createElement("option");
        o.value = c.id;
        o.textContent = c.name;
        // Offered everywhere it exists, so an engine without it is a disabled
        // row rather than a list that behaves differently per connection.
        o.disabled = !c.canRun;
        deps.connection.append(o);
      }
      const wanted = preselect.connectionId ?? activeConnectionId;
      const usable = connections.find((c) => c.id === wanted && c.canRun) ?? connections.find((c) => c.canRun);
      deps.connection.value = usable?.id ?? "";
      fillDatabases();
      if (preselect.database) deps.database.value = preselect.database;

      // Deliberately kept between openings: somebody running a folder once is
      // somebody running it again. Only the plan is thrown away, because the
      // files may have changed.
      scan = null;
      deps.preview.replaceChildren();

      // Why it is not ready is `blocker()`'s to say, so there is one place
      // that decides it and one place that paints it.
      drawPreview();
      deps.dialog.showModal();
      if (source) void rescan();
    },
  };
}
