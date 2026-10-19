# Trackers

One tracker per stage. A stage is frozen once its scope is delivered — its
tracker becomes the historical record of what was built and why, including
decisions that were corrected along the way.

| Stage | Tracker | Status |
|---|---|---|
| 0 | [POC — lightweight MySQL client](stage-0-poc.md) | 🔒 **Frozen** — complete and verified |
| 1 | [Script tabs & file open/save](stage-1-tabs-and-files.md) | 🔒 **Frozen** — complete and verified |
| 2 | [Connections](stage-2-connections.md) | 🔒 **Frozen** — complete; C3w (Windows) carried forward |
| 3 | [Schema actions & result export](stage-3-explore-and-export.md) | 🔒 **Frozen** — built; E1-E8 click-through never run, carried to Stage 4 |
| 4 | [Testability](stage-4-testability.md) | 🔒 **Frozen** — 192 UI tests on both engines; E1-E8 answered; CI + Windows carried to Stage 5 |
| 5 | [Packaging & distribution](stage-5-packaging.md) | 🔒 **Frozen** — public repo, MIT, CI green on both platforms, installers built; updater and attribution carried to Stage 6 |
| 6 | [Updates & attribution](stage-6-updates-and-attribution.md) | 🔒 **Frozen** — updater built and version taken from the tag; never run against a published release, attribution still not shipped |
| 7 | [Session persistence](stage-7-session-persistence.md) | 🔒 **Frozen** — tabs restore on connect, S1-S8 tested; hands-on confirmation carried to Stage 8 |
| 8 | [Attribution & the first real release](stage-8-attribution-and-the-first-release.md) | ✅ **Built** — R1/R3/R4/S9 confirmed by hand 2026-09-09: the updater runs, reports the new version and refuses a wrong key. R2 (the Windows double-click install) and A2 (the Windows licence tree) answered since; Stage 4 §6b's A1-A5 remain unrun |
| 9 | [Query history](stage-9-query-history.md) | ✅ **Complete** — recorded, searchable and safe; H8 confirmed by hand 2026-09-09 |
| 10 | [An assistant that writes SQL](stage-10-assistant.md) | 🚧 **Experimental** — proposes SQL, never runs it; Anthropic or any OpenAI-compatible server, local included; verified against a local model; schema is loaded before the question so the model does not guess |
| 11 | [A second engine: Elasticsearch SQL](stage-11-elasticsearch.md) | 🚧 **Built** — engine trait + capabilities; Elasticsearch runs end to end against a live cluster; reviewed hands-on 2026-09-09 (§11); connects as a restricted account since 2026-10-19 (§12) |
| 12 | [Stabilisation: result sets, idle connections, carried bugs](stage-12-stabilisation-and-result-sets.md) | ✅ **Built and confirmed** — B1-B7 verified by hand 2026-09-09 (§16); also tree/type icons, a schema filter, tabbed settings, formatted definitions and a dialect-aware linter (§13-15) |
| 13 | [Letting other tools in: an MCP server](stage-13-mcp-server.md) | 🚧 **Built** — schema and "put this query in my editor" over MCP, on loopback, off by default, nothing that executes; M1/M2/M4 confirmed by hand, M7 (every suite on both platforms) outstanding |
| 14 | [Read-only connections](stage-14-read-only-connections.md) | 🚧 **Built** — a connection can refuse writes before they are sent, deferred from Stage 2; hands-on pass outstanding |
| 15 | [Flyway migrations](stage-15-flyway-migrations.md) | ✅ **Built** — list, read, apply and repair migrations on a connection that has a Flyway project; **in use and working as of 2026-10-14**, which is what the F-checklist was for |
| 16 | [A logbook](stage-16-logbook.md) | 🚧 **Built** — what the app did, kept where a user can copy it; Settings → About → Diagnostics |
| 17 | [Flyway projects stand on their own](stage-17-flyway-projects.md) | ✅ **Built** — projects listed without a connection; each environment matched to the saved connection it is; **in use and working as of 2026-10-14** |
| 18 | [Migrations in the app's own flow](stage-18-migrations-in-the-flow.md) | ✅ **Built** — projects in the schema sidebar, the list as a tab, "Apply up to here"; the pane is deleted |
| 19 | [A result knows which statement made it](stage-19-result-sets.md) | ✅ **Built** — numbered result tabs; double-click selects the SQL that produced one |
| 20 | [The assistant through a CLI](stage-20-assistant-through-a-cli.md) | ✅ **Built** — answers from a CLI the person is already signed into (Claude Code, Copilot CLI) instead of an API key; tools stripped, nothing executed; a kept ACP session |
| 21 | [The assistant docked](stage-21-the-assistant-docked.md) | ✅ **Built** — the chat is a panel rather than a modal, so nothing behind it is inert; one-click Run into a buffer; the window reopens where it was left; every Copy says what happened |
| 22 | [Export a schema](stage-22-schema-export.md) | ✅ **Built** — a re-runnable script for selected databases, tables, views, routines, triggers and events, with or without data; one file or one per database; six facts measured first and two of them moved the design; proved by a round trip that loads the script back. Hands-on through the real UI outstanding — the recipe is in `dev/README.md` |
| 23 | [Run a folder of SQL files](stage-23-run-sql-files.md) | ✅ **Built** — select a folder, run every script in it, and write each result set out as CSV with all rows; a rail button, a progress strip that does not block the app, and a report that outlives the tab you were on; writes named before they run, a failure stops the batch; eight facts measured first and three of them changed the design; **confirmed in use 2026-10-14**, which found one bug (§9b.5) |

## Rules

- **Never edit a frozen tracker.** If something in it turns out to be wrong or
  gets superseded, record that in the *current* stage's tracker and link back.
  The value of a frozen tracker is that it says what we actually believed and
  decided at the time.
- **One stage, one file.** A stage is scoped to a coherent slice of work with
  its own milestones. When it closes, freeze it and open the next.
- **Milestones must be checkable by hand.** If you cannot demonstrate it, it is
  not a milestone.
- **Corrections are content.** When implementation proves a plan wrong, write
  down what was wrong and why — that is usually the most useful part.
