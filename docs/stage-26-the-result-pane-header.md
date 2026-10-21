# Stage 26 — a header on the result pane

**Status: done · built 2026-10-21**

## 1. The complaint

> *"Both on Flyway tab and on query is sort of inconvenient as there's no quick
> way to collapse it."*

The result pane took 45% of the window whether or not anything was in it, and
nothing gave the space back. The only way to see more SQL at once was to drag
the seam, and dragging it back afterwards is a second gesture nobody wants to
make twice a minute.

Flyway's output pane had a `Hide` button of its own since Stage 18 — but it hid
only its own body, leaving its header, the pane header and the result bar on
screen, and the SQL grid had no equivalent at all.

## 2. What was there

| Thing | How it worked |
| --- | --- |
| `#main` | `grid-template-rows: var(--editor-h, 45%) 7px 1fr` — the **editor** is sized, the pane takes the rest |
| `#hsplit` | sets `--editor-h` on drag; not persisted across launches, by Stage 12's decision |
| `#results-pane` | a flex column: job strip, job report, result tabs, grid, Flyway pane, result bar |
| `.flyway-head` | its own title, `#flyway-when`, and a `Hide`/`Show` button over `#flyway-body` |
| `#status` | the result bar's chips: `3 rows`, `12 ms`, and the warnings |

### 2.1 The measurement that changed the work

The grid is virtualised: only the rows its viewport can show exist in the DOM,
and `paintRows()` ran **on scroll alone**. So a viewport that *grew* painted
nothing new. Probed against a 400-row result:

| Viewport | Rows painted |
| --- | --- |
| 347px (the default split) | 42 |
| 585px (editor dragged small) | **42** |

The 12-row overscan above and below is the only reason that was ever invisible:
42 rows is ~880px of markup, so a pane under that height is covered by
accident. Drag a 1440p window's pane taller than that and the bottom is blank
until the wheel is touched — and **collapsing takes the viewport to zero and
back**, which no amount of overscan covers.

So this stage had to fix a pre-existing bug to land its own feature. One
`ResizeObserver` on `#grid` answers every case at once: the splitter, the
collapse, the window, and whatever pane is written next.

## 3. Decisions

1. **One header for both kinds of output.** A SQL grid and a Flyway report are
   the same pane in two costumes; two toggles would have been two habits. The
   header's title and summary follow the tab in front — `RESULTS · 3 rows ·
   12 ms` or `FLYWAY OUTPUT · migrate` — and `.flyway-head`, with its own
   `Hide` button, is **gone**.
2. **Collapsing never writes a height.** It switches `#main` to
   `1fr 7px auto` and leaves `--editor-h` exactly as it was, so reopening
   restores the size somebody dragged to. Writing a remembered height on
   collapse is how that gets lost.
3. **The whole strip is the button**, not a caret you have to hit. A 30px row
   spanning the window is what makes it a habit rather than a feature.
4. **The seam is never dead.** Dragging it while the pane is shut opens it and
   sizes it in one gesture. A handle that looks draggable and does nothing is
   the worst of the three states.
5. **A running job keeps its line.** `#job-strip` sits *outside* the part the
   header hides: collapsing the pane during a schema export or a migrate must
   not hide how far it has got — that is the main reason to collapse it then.
6. **Results arriving do not reopen it.** Asked and answered against a
   clickable prototype with both behaviours wired. Collapsing is an explicit
   act, and a run that undid it would make the gesture useless to the person
   who wants it most: somebody iterating on a long script. The header carries
   what landed instead, highlighted for 1.6s, which is an answer rather than a
   surprise.
7. **The summary shows only while shut.** Open, the result bar says it a few
   pixels lower, and a figure printed twice is a figure that can disagree with
   itself. It is also *read back off the status chips* rather than composed
   again — one formatter.
8. **Ctrl+J**, the conventional key for the panel at the bottom, free in the
   keydown handler, and now in the starter buffer's cheat sheet.
9. **Not persisted across launches**, like the splitter sizes it sits beside.

## 4. Milestones

- **R1** One header over both the grid and Flyway's output, titled for the tab
  in front.
- **R2** A click shuts the pane to that header and gives the row to the editor;
  a click reopens it at the height it had.
- **R3** The grid paints the rows a bigger pane can show.
- **R4** A shut pane says what the result bar would have said.
- **R5** A run while shut leaves it shut and says what landed.
- **R6** A running job keeps its progress line while shut.
- **R7** Ctrl+J, and a seam that opens a shut pane rather than doing nothing.

## 5. Task tracker

### Built 2026-10-21
- [x] `.res-head` in `index.html`, built from `.pane-head` and the sidebar's own
      collapse toggle · R1
- [x] `#results-body` wrapper — one thing to hide, and a real `aria-controls`
      target — with `#job-strip` deliberately outside it · R6
- [x] `#main.results-shut`, and `--editor-h` left untouched · R2
- [x] `ResizeObserver` + `ResultView.relayout()` · R3
- [x] the summary read back off `#status`, shown only while shut · R4
- [x] `noteResultsLanded()`, and the `landed` highlight · R5
- [x] Ctrl+J; the seam opens a shut pane; `STARTER_DOC` says so · R7
- [x] `.flyway-head` and `#btn-flyway-toggle` removed; `showFlywayOutput()` no
      longer decides whether it is visible
- [x] 9 UI tests in `tests/ui/result-pane.spec.ts`, 6 falsified one at a time;
      the migrations suite's collapse test moved to the new header

**A demoable prototype decided decision 6.** Both behaviours were wired into a
clickable artboard — collapse the pane, press Run, feel it — rather than argued
about in prose. The answer came back in one message.

**A falsification that passed, and the measurement behind it.** The first test
for R3 collapsed the pane and asserted the painted row count came back. It
passes with the `ResizeObserver` removed: a collapse ends on the viewport it
started from, so the count is identical either way. The property the observer
actually answers is a viewport that **grew**, so the test now drags the pane
small (painting fewer rows), drags it large, and asserts the count rose —
which fails the moment the observer goes.
