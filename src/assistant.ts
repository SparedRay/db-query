// The assistant chat.
//
// # It proposes; you execute
//
// The model has no tools and no connection — see `assistant.rs`. Every
// statement it writes lands in the editor behind an explicit click, and is run
// the same way as anything typed by hand. Nothing here executes SQL, and the
// tests assert that.
//
// # Why the SQL is parsed out of fenced blocks
//
// The system prompt asks for ```sql fences, so extraction is a matter of
// splitting rather than of understanding. That keeps the prose the model wrote
// *around* the SQL — which is usually the part that explains a join — instead of
// throwing it away to get at a structured field.

import { api, type AssistantConfig, type AssistantEvent, type ChatMessage } from "./api";

export interface AssistantDeps {
  dialog: HTMLDialogElement;
  log: HTMLElement;
  input: HTMLTextAreaElement;
  send: HTMLButtonElement;
  clear: HTMLButtonElement;
  close: HTMLElement;
  note: HTMLElement;
  /** Where questions go. Read per send, so a settings change takes effect. */
  config: () => AssistantConfig;
  /** Which connection and database to describe to the model. */
  context: () => { connectionId: string | null; db: string | null };
  /** Put SQL where the cursor is. */
  insert: (sql: string) => void;
  /** Open it as a new tab instead. */
  openInTab: (sql: string) => void;
  /** Remember that the assistant proposed this, so history can say so. */
  remember: (sql: string) => void;
}

/** A message split into the prose and the runnable blocks it contained. */
export interface Part {
  kind: "text" | "sql";
  body: string;
}

/**
 * Split an answer into prose and ```sql blocks.
 *
 * Tolerant on purpose: an unterminated fence — which is exactly what a reply cut
 * off mid-stream looks like — yields the SQL written so far rather than
 * swallowing the rest of the message. Rendering partial SQL is fine; losing the
 * end of an answer is not.
 */
export function splitAnswer(text: string): Part[] {
  const parts: Part[] = [];
  const fence = /```[ \t]*(?:sql|mysql)?[ \t]*\r?\n?/gi;
  let at = 0;
  let inSql = false;

  for (;;) {
    fence.lastIndex = at;
    const m = fence.exec(text);
    if (!m) break;
    const body = text.slice(at, m.index);
    if (body.trim()) parts.push({ kind: inSql ? "sql" : "text", body: body.replace(/\s+$/, "") });
    at = m.index + m[0].length;
    inSql = !inSql;
  }

  const tail = text.slice(at);
  if (tail.trim()) parts.push({ kind: inSql ? "sql" : "text", body: tail.replace(/\s+$/, "") });
  return parts;
}

export function createAssistant(deps: AssistantDeps) {
  /** The conversation as the API sees it. Prose and SQL both, verbatim. */
  let history: ChatMessage[] = [];
  let streaming = false;

  function bubble(cls: string): HTMLElement {
    const el = document.createElement("div");
    el.className = `chat-msg ${cls}`;
    deps.log.append(el);
    return el;
  }

  function scrollToEnd() {
    deps.log.scrollTop = deps.log.scrollHeight;
  }

  /** Render one finished assistant answer: prose, and SQL with its buttons. */
  function renderAnswer(host: HTMLElement, text: string) {
    host.replaceChildren();
    for (const part of splitAnswer(text)) {
      if (part.kind === "text") {
        const p = document.createElement("p");
        p.className = "chat-prose";
        // textContent, never innerHTML — this is model output.
        p.textContent = part.body;
        host.append(p);
        continue;
      }

      const block = document.createElement("div");
      block.className = "chat-sql";
      const pre = document.createElement("pre");
      pre.textContent = part.body;

      const actions = document.createElement("div");
      actions.className = "chat-sql-actions";

      const insert = document.createElement("button");
      insert.type = "button";
      insert.className = "mini";
      insert.textContent = "Insert at cursor";
      insert.onclick = () => {
        deps.remember(part.body);
        deps.insert(part.body);
      };

      const tab = document.createElement("button");
      tab.type = "button";
      tab.className = "mini";
      tab.textContent = "New tab";
      tab.onclick = () => {
        deps.remember(part.body);
        deps.openInTab(part.body);
      };

      actions.append(insert, tab);
      block.append(pre, actions);
      host.append(block);
    }
  }

  function setStreaming(on: boolean) {
    streaming = on;
    deps.send.disabled = on;
    deps.input.disabled = on;
    deps.send.textContent = on ? "Answering…" : "Send";
  }

  async function ask() {
    const question = deps.input.value.trim();
    if (!question || streaming) return;

    deps.input.value = "";
    const mine = bubble("from-user");
    mine.textContent = question;
    history.push({ role: "user", content: question });

    const reply = bubble("from-assistant");
    const thinking = document.createElement("p");
    thinking.className = "chat-thinking";
    thinking.hidden = true;
    const body = document.createElement("div");
    reply.append(thinking, body);

    // A visible placeholder from the first frame: a thinking model can take a
    // while before any text, and an empty bubble reads as a hang.
    body.textContent = "…";
    scrollToEnd();

    setStreaming(true);
    let answer = "";
    let failed: string | null = null;

    const onEvent = (e: AssistantEvent) => {
      if (e.type === "thinking") {
        thinking.hidden = false;
        thinking.textContent += e.delta;
      } else if (e.type === "text") {
        answer += e.delta;
        // Re-rendered per delta so fenced SQL becomes a block as it arrives.
        renderAnswer(body, answer);
      } else if (e.type === "failed") {
        failed = e.message;
      }
      scrollToEnd();
    };

    try {
      const { connectionId, db } = deps.context();
      // A copy: the request is a snapshot of the conversation, and `history` is
      // still being mutated by the turn that is in flight.
      await api.assistantSend(deps.config(), connectionId, db, [...history], onEvent);
    } catch (err) {
      // The request never started — nothing was shown, so this replaces it.
      failed = String(err);
    } finally {
      setStreaming(false);
    }

    if (answer.trim()) {
      history.push({ role: "assistant", content: answer });
    } else {
      // Nothing came back, so the question must not stay in the transcript as
      // though it had been answered — the next turn would be built on a lie.
      history.pop();
    }

    if (failed) {
      const err = document.createElement("p");
      err.className = "chat-error";
      err.textContent = failed;
      reply.append(err);
    } else if (!answer.trim()) {
      body.textContent = "No answer came back.";
    }
    thinking.hidden = thinking.textContent!.trim() === "";
    scrollToEnd();
    deps.input.focus();
  }

  deps.send.onclick = () => void ask();
  deps.clear.onclick = () => {
    history = [];
    deps.log.replaceChildren();
  };
  deps.close.addEventListener("click", () => deps.dialog.close());

  // Enter sends; Shift+Enter is a newline. A chat box that needs a mouse to
  // send is a chat box nobody uses twice.
  deps.input.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void ask();
    }
  });

  return {
    async open() {
      if (deps.dialog.open) {
        deps.input.focus();
        return;
      }
      const config = deps.config();
      const status = await api
        .assistantStatus(config.provider, config.baseUrl)
        .catch(() => null);
      const ready = (status?.ready ?? false) && config.model.trim() !== "";

      deps.note.textContent = !ready
        ? status && !status.ready
          ? "No API key set. Add one in Settings to use the assistant."
          : "No model set. Choose one in Settings."
        : `${config.model} · ` +
          (status?.local
            ? "this model runs on your machine — nothing leaves it."
            : "your schema's table and column names are sent with each question; row data never is.");

      deps.send.disabled = !ready;
      deps.input.disabled = !ready;
      deps.dialog.showModal();
      if (ready) deps.input.focus();
    },
  };
}
