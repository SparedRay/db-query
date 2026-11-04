//! Which tables are worth describing to the assistant.
//!
//! # Why this exists
//!
//! The prompt could never carry every column of a large database, so it carried
//! the first 60 tables **alphabetically** — a limit inherited from
//! [`crate::schema::TABLE_DETAIL_BUDGET`], which bounds round trips on an
//! engine that describes one table at a time and has nothing to do with the
//! size of a prompt. On a 400-table server that meant the model was handed
//! `account_*` through `customer_*` and told the other 340 tables existed.
//!
//! Stage 30's field report is what it looks like from the outside: asked for
//! enrolled items with their supplier and reserve balances, the assistant
//! declined, because `enrolled_item`, `enrolled_party`, `supplier_status` and
//! `party_status` were all past the letter C.
//!
//! Nothing was missing from the cache — `schema::warm` holds every column of
//! every table. The prompt was choosing the wrong ones. So this orders them,
//! and the renderer spends its budget down that order.
//!
//! # An ordering, not a score
//!
//! A weighted sum would need weights, and weights would need a corpus of real
//! questions to fit them against. We have one question. A number nobody
//! measured, written as though somebody had, is worse than an ordering that
//! says plainly what it prefers — so this is a lexicographic sort key and every
//! term in it is a yes-or-no or a count.
//!
//! The one place a count appears is how many of the question's words a table's
//! columns match, and it needs no rarity weighting to behave: a word like
//! `name` that matches three hundred tables lifts all three hundred equally and
//! therefore decides nothing between them, leaving the keys below it to. A word
//! like `reserve` matches one, which is the whole point — "reserve balance",
//! "original supplier" and "servicing status" name no table at all.
//!
//! # What it never does
//!
//! It never drops a table. Everything the database has is named in the prompt
//! whatever this decides; the ranking only chooses who is described in full.
//! A backup table is demoted, and still listed — if you asked about
//! `orders_backup` by name, you meant `orders_backup`.
//!
//! And **the editor's text never reaches the prompt.** It is read here, turned
//! into a set of words, and dropped. The promise the chat header makes is that
//! table and column names are sent and row data never is; a `WHERE` clause from
//! somebody's buffer is neither, so it ranks and goes no further.

use std::collections::{HashMap, HashSet};

use crate::schema::DbSchema;

/// Everything the ranking may look at.
#[derive(Default)]
pub struct Signals<'a> {
    /// What the user asked, when the prompt is built per question.
    ///
    /// `None` on the one path that cannot use it: a kept CLI session is primed
    /// with the system prompt once and reused only while it is byte-identical,
    /// so a prompt that moved with every question would restart the process
    /// every turn — 5.3-11.7s against 1.85-2.66s, measured in Stage 20. That
    /// path ranks on the standing signals instead and gets a budget large
    /// enough not to need the question. Stage 30 §4.3.
    pub question: Option<&'a str>,
    /// The SQL in the editor. **Ranks only** — see this module's header.
    pub editor: Option<&'a str>,
    /// SQL lately run on this connection, newest first.
    pub history: &'a [String],
    /// The user's own notes, which say both which tables they cared about and
    /// which tables those join to.
    pub dictionary: Option<&'a crate::dictionary::Dictionary>,
}

/// Words that say nothing about which table somebody means.
///
/// English function words and the SQL that surrounds a name. Short rather than
/// exhaustive: a word left in costs a table one point of a count that common
/// words cannot decide anyway, where a real name wrongly listed here costs the
/// signal that would have found it.
const NOISE: &[&str] = &[
    "all", "and", "any", "are", "but", "every", "for", "from", "give", "group", "has", "have",
    "how", "inner", "into", "join", "left", "like", "list", "many", "me", "much", "not", "null",
    "one", "only", "order", "outer", "please", "query", "right", "row", "rows", "select", "show",
    "sql", "table", "tables", "that", "the", "their", "them", "then", "this", "value", "values",
    "want", "was", "were", "what", "when", "where", "which", "who", "why", "with", "would",
];

/// Names that say "this is a copy of something else".
const COPIES: &[&str] = &[
    "archive",
    "archived",
    "backup",
    "bak",
    "copy",
    "dead",
    "deprecated",
    "obsolete",
    "old",
    "orig",
    "temp",
    "tmp",
    "unused",
];

/// Endings that are not a plural `s`.
///
/// **A length guard is not enough, and the test that said so caught it:**
/// `status` is six letters and ends in `s`, so stripping it gave `statu` and
/// the single most common word in a schema stopped matching anything. These
/// are the Latin and double endings where a final `s` belongs to the word —
/// `status`, `bonus`, `basis`, `analysis`, `address`, `chaos`. `as` is
/// deliberately absent: `areas` and `schemas` are ordinary plurals.
const NOT_PLURAL: &[&str] = &["us", "is", "os", "ss"];

/// Fold a word to the form a different spelling of the same thing shares.
///
/// Lower case, and a crude singular: a question says "every enrolled item"
/// where the table is `enrolled_items`, and "list the parties" where it is
/// `parties`.
fn fold(word: &str) -> String {
    let w = word.to_lowercase();
    if w.ends_with("ies") && w.len() > 4 {
        return format!("{}y", &w[..w.len() - 3]);
    }
    // `statuses` to `status`, `addresses` to `address`, `boxes` to `box`.
    if (w.ends_with("ses") || w.ends_with("xes") || w.ends_with("hes")) && w.len() > 5 {
        return w[..w.len() - 2].to_string();
    }
    if w.ends_with('s') && w.len() > 3 && !NOT_PLURAL.iter().any(|e| w.ends_with(e)) {
        return w[..w.len() - 1].to_string();
    }
    w
}

/// The identifiers a text contains, **whole**: `enrolled_item` as one name
/// rather than as the words `enrolled` and `debt`.
///
/// This is what lets naming a table outrank merely using its words. Asking to
/// "select from parties_backup" uses the word `party_*`, so it also names
/// `parties` by [`names_it`]'s rule — and `parties` is alphabetically first and
/// not a copy, so it won. Spelling a table out is a stronger thing to have done
/// than sharing its vocabulary, and it gets its own key.
fn raw(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| t.len() >= 3)
        .map(str::to_lowercase)
        .collect()
}

/// Every word in a piece of text, folded.
///
/// Splits on anything that is not a letter or a digit, then splits the pieces
/// again on `_` and on camelCase boundaries — so `reserve_balance` and
/// `reserveBalance` both yield `reserve` and `balance`, and a question asking for
/// "the reserve balance" matches either.
pub fn words(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for raw in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
        for piece in raw.split('_') {
            for part in camel_parts(piece) {
                if part.len() < 3 {
                    continue;
                }
                let folded = fold(&part);
                if NOISE.contains(&folded.as_str()) {
                    continue;
                }
                out.insert(folded);
            }
        }
    }
    out
}

/// `reserveBalance` to `reserve`, `balance`. A run of capitals stays whole, so
/// `partyID` yields `party_*` and `id` rather than one letter at a time.
fn camel_parts(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut start = 0usize;
    for i in 1..chars.len() {
        let boundary = chars[i].is_uppercase()
            && (chars[i - 1].is_lowercase()
                || chars[i - 1].is_ascii_digit()
                || (i + 1 < chars.len() && chars[i + 1].is_lowercase()));
        if boundary {
            out.push(chars[start..i].iter().collect());
            start = i;
        }
    }
    out.push(chars[start..].iter().collect());
    out.retain(|p: &String| !p.is_empty());
    out
}

/// Does this text mean this table?
///
/// Either it used the whole name — `enrolled_item`, which folds to the same set
/// of words — or it used every word of it: "every enrolled item" names
/// `enrolled_item`, while "every debt" does not name `enrolled_item_detail`.
///
/// Requiring *every* word rather than any is what keeps a schema full of
/// `client_*` tables from all matching the word "party". The table actually
/// called `parties` matches it; the forty tables that merely begin with it do
/// not.
fn names_it(table: &str, said: &HashSet<String>) -> bool {
    if said.is_empty() {
        return false;
    }
    let mine = words(table);
    !mine.is_empty() && mine.iter().all(|w| said.contains(w))
}

/// How many distinct words of the question this table's columns account for.
fn column_hits(cols: &[crate::schema::ColumnInfo], asked: &HashSet<String>) -> usize {
    if asked.is_empty() {
        return 0;
    }
    let mut mine: HashSet<String> = HashSet::new();
    for c in cols {
        mine.extend(words(&c.name));
    }
    asked.iter().filter(|w| mine.contains(*w)).count()
}

/// Does the name look like a copy of another table rather than a table?
///
/// A word of the name saying so, or a trailing run of four or more digits —
/// `orders_2019`, `orders_20240131`. Not `v2` or `_2`: a short number is as
/// likely to be part of the real name.
pub fn looks_like_a_copy(name: &str) -> bool {
    let lower = name.to_lowercase();
    let mut segments = lower.split(|c: char| c == '_' || c == '-').peekable();
    let mut last = "";
    while let Some(seg) = segments.next() {
        if COPIES.contains(&seg) {
            return true;
        }
        if segments.peek().is_none() {
            last = seg;
        }
    }
    last.len() >= 4 && last.chars().all(|c| c.is_ascii_digit())
}

/// Table names a dictionary mentions: described by one, or pointed at by
/// another table's `join`.
///
/// Following the joins is free — the expressions are already parsed — and it is
/// what makes one note useful for the tables around it: a note on
/// `enrolled_party` whose `join` names `party_status` brings `party_status`
/// along, which is exactly the pair the field report needed.
fn from_dictionary(
    db: &str,
    tables: &[crate::schema::TableRef],
    dict: &crate::dictionary::Dictionary,
) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut joined: HashSet<String> = HashSet::new();
    for t in tables {
        if let Some(said) = dict.table(db, &t.name) {
            out.insert(t.name.to_lowercase());
            if let Some(join) = said.join.as_deref() {
                // A join expression is prose with identifiers in it. Every word
                // of it is a candidate; only the ones that are real table names
                // survive the filter below.
                for raw in join.split(|c: char| !c.is_alphanumeric() && c != '_') {
                    if !raw.is_empty() {
                        joined.insert(raw.to_lowercase());
                    }
                }
            }
        }
    }
    for t in tables {
        if joined.contains(&t.name.to_lowercase()) {
            out.insert(t.name.to_lowercase());
        }
    }
    out
}

/// Which of a question's words this schema has a name for, and how many it has
/// not.
///
/// **For the log, and safe there by construction.** Measuring the ranking
/// against a real server needs to show what it read, and the first instinct —
/// log the question's words — can smuggle a secret past `logbook`'s scrubber:
/// somebody typing "connect with password hunter2" yields the bare word
/// `hunter2`, and the scrubber looks for `password=` and `password:`, not for a
/// word in a list.
///
/// So only the words the schema itself accounts for are named, which cannot be
/// anything but schema vocabulary — the same argument that keeps the editor's
/// text out of the prompt. The rest are a count.
///
/// It is also the better diagnostic. The question worth asking of a bad answer
/// is "did my words land at all?", and a vocabulary this returns empty says
/// the folding is wrong (`status` arriving as `statu`, which happened) where a
/// full one says the ordering is.
pub fn recognised(schema: &DbSchema, asked: &HashSet<String>) -> (Vec<String>, usize) {
    if asked.is_empty() {
        return (Vec::new(), 0);
    }
    let mut known: HashSet<String> = HashSet::new();
    for t in schema.tables.as_deref().unwrap_or_default() {
        known.extend(words(&t.name));
    }
    for cols in schema.columns.values() {
        for c in cols {
            known.extend(words(&c.name));
        }
    }
    let mut hit: Vec<String> = asked.intersection(&known).cloned().collect();
    hit.sort();
    (hit.clone(), asked.len() - hit.len())
}

/// The tables of `db`, most worth describing first.
///
/// The key, in order — see this module's header for why it is an ordering:
///
/// 1. **The question spells the table out.** Nothing beats being asked for by
///    name.
/// 2. **The question uses every word of its name.** "every enrolled item" for
///    `enrolled_item`.
/// 3. **The editor spells it out**, and then 4. **uses its words.** A
///    deliberate act about the query in hand, and stronger than any inference
///    from a column — but weaker than what was actually asked.
/// 5. **How many of the question's words its columns match**, most first.
/// 6. **The dictionary describes it**, or another table's `join` points at it.
///    Somebody wrote it down.
/// 7. **Recent history uses its words.**
/// 8. **It does not look like a copy.** Below everything above, so a backup
///    somebody asked for by name still wins.
/// 9. **Alphabetically**, so one question twice is one prompt twice — which a
///    kept CLI session depends on.
pub fn order(db: &str, schema: &DbSchema, signals: &Signals) -> Vec<String> {
    let tables = schema.tables.as_deref().unwrap_or_default();
    if tables.is_empty() {
        return Vec::new();
    }

    let asked = signals.question.map(words).unwrap_or_default();
    let asked_by_name = signals.question.map(raw).unwrap_or_default();
    let typed = signals.editor.map(words).unwrap_or_default();
    let typed_by_name = signals.editor.map(raw).unwrap_or_default();
    let recent: HashSet<String> = signals.history.iter().flat_map(|sql| words(sql)).collect();
    let described = signals
        .dictionary
        .map(|d| from_dictionary(db, tables, d))
        .unwrap_or_default();

    let empty: Vec<crate::schema::ColumnInfo> = Vec::new();
    type Key<'a> = (
        bool,
        bool,
        bool,
        bool,
        std::cmp::Reverse<usize>,
        bool,
        bool,
        bool,
        String,
        &'a str,
    );
    let mut keyed: Vec<Key> = Vec::with_capacity(tables.len());
    let mut columns: HashMap<&str, &Vec<crate::schema::ColumnInfo>> = HashMap::new();
    for (name, cols) in &schema.columns {
        columns.insert(name.as_str(), cols);
    }

    for t in tables {
        let cols = columns.get(t.name.as_str()).copied().unwrap_or(&empty);
        let lower = t.name.to_lowercase();
        keyed.push((
            // Negated throughout: `false` sorts first, and the thing we want
            // first is the thing that is true.
            !asked_by_name.contains(&lower),
            !names_it(&t.name, &asked),
            !typed_by_name.contains(&lower),
            !names_it(&t.name, &typed),
            std::cmp::Reverse(column_hits(cols, &asked)),
            !described.contains(&lower),
            !names_it(&t.name, &recent),
            looks_like_a_copy(&t.name),
            lower,
            t.name.as_str(),
        ));
    }
    keyed.sort();
    keyed.into_iter().map(|k| k.9.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ColumnInfo, TableRef};

    fn col(name: &str) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            data_type: "varchar(64)".into(),
            nullable: true,
            key: None,
        }
    }

    /// The schema from the field report, with enough neighbours to put the
    /// tables it needs well past the alphabetical 60.
    fn report_schema() -> DbSchema {
        let mut tables: Vec<TableRef> = Vec::new();
        let mut columns: HashMap<String, Vec<ColumnInfo>> = HashMap::new();

        let real: &[(&str, &[&str])] = &[
            (
                "enrolled_item",
                &["id", "enrolled_party_id", "supplier_id", "current_balance"],
            ),
            ("enrolled_party", &["id", "party_id", "status_id"]),
            ("parties", &["id", "first_name", "last_name"]),
            ("party_status", &["id", "servicing_status"]),
            ("supplier_status", &["id", "label"]),
            ("Supplier", &["id", "original_supplier", "name"]),
            (
                "external_holding_account",
                &["id", "reserve_balance", "party_id"],
            ),
        ];
        for (name, cols) in real {
            tables.push(TableRef {
                name: (*name).into(),
                kind: "BASE TABLE".into(),
            });
            columns.insert(
                (*name).to_string(),
                cols.iter().map(|c| col(c)).collect::<Vec<_>>(),
            );
        }
        // Filler that sorts before "enrolled_*", which is the whole of the bug.
        for i in 0..120 {
            let name = format!("account_ledger_{i:03}");
            tables.push(TableRef {
                name: name.clone(),
                kind: "BASE TABLE".into(),
            });
            columns.insert(name, vec![col("id"), col("amount")]);
        }
        tables.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        DbSchema {
            tables: Some(tables),
            columns,
            routines: None,
        }
    }

    const QUESTION: &str = "for one member, list every enrolled item with the party name, \
                            servicing status, supplier status, current and original supplier, \
                            balances and reserve balance";

    /// The whole point of the stage. Every table their query needed must be
    /// near the front, where the budget reaches.
    #[test]
    fn the_question_brings_its_own_tables_to_the_front() {
        let schema = report_schema();
        let ranked = order(
            "appdata",
            &schema,
            &Signals {
                question: Some(QUESTION),
                ..Default::default()
            },
        );
        let top: Vec<&str> = ranked.iter().take(7).map(String::as_str).collect();
        for want in [
            "enrolled_item",
            "enrolled_party",
            "parties",
            "party_status",
            "supplier_status",
            "Supplier",
            "external_holding_account",
        ] {
            assert!(
                top.contains(&want),
                "{want} is not in the first seven: {top:?}"
            );
        }
    }

    /// A column name is enough. "reserve balance" names no table at all, and is
    /// the only way to find `external_holding_account`.
    #[test]
    fn a_column_the_question_names_finds_its_table() {
        let schema = report_schema();
        let ranked = order(
            "appdata",
            &schema,
            &Signals {
                question: Some("what is the reserve balance"),
                ..Default::default()
            },
        );
        assert_eq!(
            ranked.first().map(String::as_str),
            Some("external_holding_account"),
            "{:?}",
            &ranked[..3]
        );
    }

    /// Unranked, the alphabet puts 120 ledger tables in front of every table
    /// the question was about — which is the bug as it shipped.
    #[test]
    fn without_a_question_the_alphabet_is_all_there_is() {
        let schema = report_schema();
        let ranked = order("appdata", &schema, &Signals::default());
        assert!(
            ranked[0].starts_with("account_ledger_"),
            "unranked order moved: {:?}",
            &ranked[..3]
        );
        let at = ranked.iter().position(|n| n == "enrolled_item").unwrap();
        assert!(at > 100, "enrolled_item was already near the front at {at}");
    }

    #[test]
    fn the_editor_outranks_a_single_column_coincidence() {
        let schema = report_schema();
        let ranked = order(
            "appdata",
            &schema,
            &Signals {
                question: Some("which id should I use"),
                editor: Some("SELECT * FROM supplier_status WHERE id = 1"),
                ..Default::default()
            },
        );
        assert_eq!(ranked.first().map(String::as_str), Some("supplier_status"));
    }

    /// Following a `join` is what makes one note useful for the table beside
    /// it: the report's note was on `enrolled_party`, and the table it saves
    /// you from getting wrong is `party_status`.
    #[test]
    fn a_join_expression_brings_the_table_it_points_at() {
        let schema = report_schema();
        let dict = crate::dictionary::parse(
            "[tables.enrolled_party]\n\
             note = \"one party's enrolment\"\n\
             join = \"enrolled_party.status_id = party_status.id\"\n",
        )
        .unwrap()
        .dictionary;
        let ranked = order(
            "appdata",
            &schema,
            &Signals {
                dictionary: Some(&dict),
                ..Default::default()
            },
        );
        let top: Vec<&str> = ranked.iter().take(2).map(String::as_str).collect();
        assert!(top.contains(&"enrolled_party"), "{top:?}");
        assert!(top.contains(&"party_status"), "{top:?}");
    }

    #[test]
    fn a_copy_sinks_below_a_table_nobody_mentioned() {
        let mut schema = report_schema();
        for name in ["parties_backup", "parties_2019", "parties_old"] {
            schema.tables.as_mut().unwrap().push(TableRef {
                name: name.into(),
                kind: "BASE TABLE".into(),
            });
            schema.columns.insert(name.to_string(), vec![col("id")]);
        }
        let ranked = order("appdata", &schema, &Signals::default());
        let last: Vec<&str> = ranked.iter().rev().take(3).map(String::as_str).collect();
        for name in ["parties_backup", "parties_2019", "parties_old"] {
            assert!(last.contains(&name), "{name} did not sink: {last:?}");
        }
    }

    /// Demoted, never dropped — and never below the question.
    #[test]
    fn a_copy_you_asked_for_by_name_still_wins() {
        let mut schema = report_schema();
        schema.tables.as_mut().unwrap().push(TableRef {
            name: "parties_backup".into(),
            kind: "BASE TABLE".into(),
        });
        schema
            .columns
            .insert("parties_backup".to_string(), vec![col("id")]);
        let ranked = order(
            "appdata",
            &schema,
            &Signals {
                question: Some("select from parties_backup"),
                ..Default::default()
            },
        );
        assert_eq!(ranked.first().map(String::as_str), Some("parties_backup"));
    }

    /// Requiring every word of a name is what keeps a `client_*` schema from
    /// matching the word "party" forty times over.
    #[test]
    fn a_shared_first_word_does_not_name_a_table() {
        let said = words("list the parties");
        assert!(names_it("parties", &said));
        assert!(!names_it("party_status", &said));
        assert!(!names_it("enrolled_party", &said));
    }

    #[test]
    fn plurals_and_camel_case_fold_to_the_same_words() {
        assert!(words("reserveBalance").contains("reserve"));
        assert!(words("reserveBalance").contains("balance"));
        assert!(words("reserve_balances").contains("balance"));
        assert!(words("companies").contains("company"));
        // Not eaten: `status` must not fold to `statu`.
        assert!(words("the status").contains("status"));
        assert!(words("statuses").contains("status"));
    }

    #[test]
    fn a_short_numeric_suffix_is_not_a_copy() {
        assert!(looks_like_a_copy("orders_2019"));
        assert!(looks_like_a_copy("orders_20240131"));
        assert!(looks_like_a_copy("orders_backup"));
        assert!(looks_like_a_copy("ORDERS_BAK"));
        assert!(!looks_like_a_copy("address_v2"));
        assert!(!looks_like_a_copy("order_line_2"));
        assert!(!looks_like_a_copy("enrolled_item"));
    }

    /// The order must be total, or a kept CLI session is restarted by nothing
    /// more than a `HashMap`'s iteration order.
    #[test]
    fn the_same_question_twice_is_the_same_order() {
        let schema = report_schema();
        let once = order(
            "appdata",
            &schema,
            &Signals {
                question: Some(QUESTION),
                ..Default::default()
            },
        );
        let twice = order(
            "appdata",
            &schema,
            &Signals {
                question: Some(QUESTION),
                ..Default::default()
            },
        );
        assert_eq!(once, twice);
    }

    /// **M7.** The editor's text ranks and goes no further. It cannot reach the
    /// prompt because the only thing that leaves here is a list of names the
    /// schema already has — which is what this asserts, rather than trusting
    /// the call site to keep them apart.
    #[test]
    fn nothing_but_a_table_name_comes_out_of_the_ranking() {
        let schema = report_schema();
        let known: HashSet<&str> = schema
            .tables
            .as_deref()
            .unwrap()
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        let ranked = order(
            "appdata",
            &schema,
            &Signals {
                question: Some("who has the balance"),
                editor: Some(
                    "SELECT * FROM enrolled_item WHERE ssn = '123-45-6789' AND secret = 'hunter2'",
                ),
                history: &["SELECT card FROM parties WHERE pan = '4111111111111111'".to_string()],
                ..Default::default()
            },
        );
        for name in &ranked {
            assert!(known.contains(name.as_str()), "{name} is not a table");
        }
        assert!(!ranked.iter().any(|n| n.contains("hunter2")));
        assert!(!ranked.iter().any(|n| n.contains("4111")));
    }

    // ------------------------------------- what the log is allowed to carry

    /// **The reason `recognised` exists rather than logging the words.** A
    /// question can contain anything, and `logbook`'s scrubber looks for
    /// `password=` and `password:` — not for a bare word in a list. So the log
    /// names only what the schema accounts for, which cannot be a secret
    /// unless somebody named a column after one.
    #[test]
    fn only_words_the_schema_has_a_name_for_are_named() {
        let schema = report_schema();
        let asked = words("the reserve balance, and connect with password hunter2 token ab9f3e");
        let (landed, missed) = recognised(&schema, &asked);

        assert!(landed.contains(&"reserve".to_string()), "{landed:?}");
        assert!(landed.contains(&"balance".to_string()), "{landed:?}");
        for leak in ["hunter2", "ab9f3e", "password", "token", "connect"] {
            assert!(
                !landed.iter().any(|w| w == leak),
                "{leak} reached the log: {landed:?}"
            );
        }
        // The rest are a count, which is all they can safely be.
        assert!(
            missed >= 4,
            "expected the unknown words to be counted: {missed}"
        );
    }

    /// The diagnostic this is for: words that landed nowhere mean the
    /// vocabulary did not arrive, which is a different bug from a bad order.
    #[test]
    fn a_question_the_schema_knows_nothing_about_lands_nothing() {
        let schema = report_schema();
        let (landed, missed) = recognised(&schema, &words("quarterly revenue by region"));
        assert!(landed.is_empty(), "{landed:?}");
        assert_eq!(missed, 3);
    }

    /// `status` folding to `statu` was caught by its own test, but this is the
    /// shape it would have shown up as in the field: a word plainly in the
    /// schema, reported as not recognised.
    #[test]
    fn a_folding_bug_shows_up_as_a_word_that_did_not_land() {
        let schema = report_schema();
        let (landed, _) = recognised(&schema, &words("servicing status"));
        assert!(landed.contains(&"status".to_string()), "{landed:?}");
        assert!(landed.contains(&"servicing".to_string()), "{landed:?}");
    }

    /// Nothing is dropped, whatever the signals say.
    #[test]
    fn every_table_is_still_in_the_order() {
        let schema = report_schema();
        let ranked = order(
            "appdata",
            &schema,
            &Signals {
                question: Some(QUESTION),
                ..Default::default()
            },
        );
        assert_eq!(ranked.len(), schema.tables.as_ref().unwrap().len());
    }
}
