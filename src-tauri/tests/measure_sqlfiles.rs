//! Stage 23 §9 — the six facts the design branches on, measured before it is
//! built.
//!
//! Stage 22 measured six and two of them moved the design, which is why this
//! exists before the feature rather than after it. The three pure ones run
//! offline:
//!
//!     cargo test --test measure_sqlfiles
//!
//! The three that need a server are `#[ignore]`d:
//!
//!     mise run db-up && mise run export-probe-up
//!     cargo test --test measure_sqlfiles -- --ignored --nocapture --test-threads=1
//!
//! Every one of them prints what it found. A measurement nobody can read is a
//! measurement nobody will check.

use db_query_lib::split;

// ------------------------------------------------- §9.1 `split` on real files

/// **A file is not an editor buffer.** Four things are ordinary in a file and
/// absent from the editor: no trailing semicolon, a UTF-8 BOM, CRLF line
/// endings, and a comment after the last statement. Each of them could make the
/// statement count in the preview wrong, which is the number the whole
/// confirmation rests on.
#[test]
fn measure_split_on_file_shapes() {
    let cases: Vec<(&str, String)> = vec![
        ("two plain statements", "SELECT 1;\nSELECT 2;\n".to_string()),
        ("no trailing semicolon", "SELECT 1;\nSELECT 2".to_string()),
        ("CRLF", "SELECT 1;\r\nSELECT 2;\r\n".to_string()),
        ("UTF-8 BOM", format!("\u{feff}SELECT 1;\nSELECT 2;\n")),
        (
            "trailing comment after the last statement",
            "SELECT 1;\nSELECT 2;\n-- done\n".to_string(),
        ),
        (
            "leading comment before the first",
            "-- header\nSELECT 1;\n".to_string(),
        ),
        (
            "a bare semicolon between",
            "SELECT 1;;\nSELECT 2;".to_string(),
        ),
        (
            "block comment between",
            "SELECT 1;\n/* why */\nSELECT 2;".to_string(),
        ),
    ];

    println!("\n§9.1 split() on file shapes");
    for (name, sql) in &cases {
        let out = split::split(sql);
        let texts: Vec<&str> = out
            .statements
            .iter()
            .map(|s| &sql[s.start..s.end])
            .collect();
        println!("  {name:45} -> {} {:?}", texts.len(), texts);
    }
}

// ------------------------------------------------- §9.2 a file of only comments

/// **Zero statements has to be a reportable answer.** The preview says
/// "nothing to run" for such a file; if `split` returns one empty span instead
/// of none, the runner would send an empty string to the server and the
/// preview would count a statement that does not exist.
#[test]
fn measure_a_file_with_nothing_to_run() {
    let cases = [
        ("only a line comment", "-- nothing here\n"),
        ("only a block comment", "/* nothing\n   here */\n"),
        ("empty", ""),
        ("whitespace only", "\n\n   \n"),
        ("a lone semicolon", ";"),
        ("comment then semicolon", "-- hm\n;\n"),
    ];

    println!("\n§9.2 files with nothing to run");
    for (name, sql) in cases {
        let out = split::split(sql);
        let texts: Vec<&str> = out
            .statements
            .iter()
            .map(|s| &sql[s.start..s.end])
            .collect();
        println!("  {name:25} -> {} {:?}", texts.len(), texts);
    }
}

// ------------------------------------------------- §9.6 byte offset to a line

/// **The confirmation and every error say `report.sql:42`.** `StatementSpan`
/// carries byte offsets, so the line has to be derived.
///
/// Two findings, and the second changes the design. Counting newline *bytes* in
/// the prefix is right, and spans land on character boundaries — so an accent
/// in a comment does not panic the slice. But **a span starts at the leading
/// comment, not at the SQL** (§9.1 saw the same thing), so the naive line is
/// the comment's line and the naive text to show is `-- header` rather than the
/// `INSERT` the confirmation is asking about.
///
/// Hence `first_code_byte`, derived from `split::mask_noncode` rather than by
/// re-implementing comment skipping — the masker already knows where code is,
/// and a second opinion about it is a second thing to get wrong.
#[test]
fn measure_offset_to_line_with_multibyte_text() {
    fn line_of(text: &str, offset: usize) -> usize {
        text.as_bytes()[..offset]
            .iter()
            .filter(|b| **b == b'\n')
            .count()
            + 1
    }

    /// The offset of the first byte of actual code in `span_text`, relative to
    /// its start. `None` when the span is all comment and whitespace.
    fn first_code_byte(span_text: &str) -> Option<usize> {
        let masked = split::mask_noncode(span_text);
        masked.bytes().position(|b| !b.is_ascii_whitespace())
    }

    let sql = "-- héllo ★ 表 🔐 a comment with four multi-byte characters\n\
               SELECT 1;\n\
               -- another: ñ\n\
               SELECT 2;\n";

    println!("\n§9.6 byte offset -> line number");
    println!(
        "  the text is {} bytes, {} chars",
        sql.len(),
        sql.chars().count()
    );
    let out = split::split(sql);
    for s in &out.statements {
        let text = &sql[s.start..s.end];
        let on_boundary = sql.is_char_boundary(s.start) && sql.is_char_boundary(s.end);
        assert!(on_boundary, "a span landed mid-character");

        let code = first_code_byte(text).expect("these spans all contain code");
        println!(
            "  span at {:>3} (line {}) · code at {:>3} (line {}) · shows {:?}",
            s.start,
            line_of(sql, s.start),
            s.start + code,
            line_of(sql, s.start + code),
            &text[code..],
        );
    }

    // **The measured truth, not what I expected.** The first span begins at
    // offset 0 because it owns the comment above it, so the naive line is 1 —
    // and the line somebody wants to be told is 2.
    let first = out.statements[0];
    assert_eq!(
        line_of(sql, first.start),
        1,
        "the span starts at the comment"
    );
    let code = first_code_byte(&sql[first.start..first.end]).unwrap();
    assert_eq!(
        line_of(sql, first.start + code),
        2,
        "the code starts on the line somebody would be told about"
    );
    assert_eq!(&sql[first.start + code..first.end], "SELECT 1");

    // The second, where the multi-byte characters are all above it: the proof
    // that counting bytes rather than chars is the right arithmetic.
    let second = out.statements[1];
    let code2 = first_code_byte(&sql[second.start..second.end]).unwrap();
    assert_eq!(line_of(sql, second.start + code2), 4);
    assert_eq!(&sql[second.start + code2..second.end], "SELECT 2");

    // And the case the preview has to label "nothing to run" rather than
    // sending to the server.
    assert_eq!(first_code_byte("-- all comment\n"), None);
    assert_eq!(first_code_byte("/* and this */\n  \n"), None);
}

// ------------------------------------------------------------- the live three

const URL: &str = "mysql://root:devpassword@127.0.0.1:3306/poc";

#[cfg(test)]
mod live {
    use super::*;
    use sqlx::{Connection, Executor, MySqlConnection, Row};
    use std::io::Write;

    async fn connect() -> MySqlConnection {
        MySqlConnection::connect(URL)
            .await
            .expect("connect failed — is the fixture up? `mise run db-up`")
    }

    /// **§9.3 — the fact that decides one connection or many.**
    ///
    /// sqlx holds the connection for the life of a `fetch` stream. The runner
    /// wants to stream a result to CSV and then run the *next* statement on the
    /// same connection, so three things have to be true, and the third is the
    /// one Stop depends on:
    ///
    ///   * a stream read to the end, then another statement — fine, surely;
    ///   * a stream **dropped early** (the plain case), then another statement;
    ///   * a stream dropped early *and the connection reused immediately*,
    ///     which is what cancelling mid-result does.
    ///
    /// If an abandoned stream leaves the connection unusable, the job needs a
    /// connection per statement and §5 changes.
    #[tokio::test]
    #[ignore = "needs the live fixture"]
    async fn measure_reusing_a_connection_after_a_stream() {
        use futures_util::StreamExt;

        println!("\n§9.3 reusing a connection after a result stream");
        let mut conn = connect().await;

        // 1. Read to the end, then another statement.
        {
            let mut n = 0;
            let mut stream = sqlx::query("SELECT id FROM big LIMIT 500").fetch(&mut conn);
            while let Some(r) = stream.next().await {
                r.unwrap();
                n += 1;
            }
            drop(stream);
            println!("  drained {n} rows");
        }
        let after: i64 = sqlx::query("SELECT COUNT(*) FROM users")
            .fetch_one(&mut conn)
            .await
            .map(|r| r.try_get(0).unwrap())
            .expect("a statement after a drained stream");
        println!("  statement after a drained stream: ok ({after} users)");

        // 2. Abandon it after a handful of rows — the cancel case.
        {
            let mut n = 0;
            let mut stream = sqlx::query("SELECT id FROM big").fetch(&mut conn);
            while let Some(r) = stream.next().await {
                r.unwrap();
                n += 1;
                if n == 5 {
                    break;
                }
            }
            println!("  abandoned after {n} of 7500 rows");
        }
        let res: Result<i64, _> = sqlx::query("SELECT COUNT(*) FROM users")
            .fetch_one(&mut conn)
            .await
            .map(|r| r.try_get::<i64, _>(0).unwrap());
        match &res {
            Ok(v) => println!("  statement after an ABANDONED stream: ok ({v} users)"),
            Err(e) => println!("  statement after an ABANDONED stream: FAILED — {e}"),
        }
        assert!(
            res.is_ok(),
            "an abandoned stream left the connection unusable, so §5 needs one \
             connection per statement"
        );

        // 3. And again, to be sure the connection is not merely limping.
        let twice: i64 = sqlx::query("SELECT 1 + 1")
            .fetch_one(&mut conn)
            .await
            .map(|r| r.try_get(0).unwrap())
            .unwrap();
        println!("  and still usable after that: {twice}");
    }

    /// **§9.7 — does a BOM reach the server, and does the server mind?**
    ///
    /// §9.1 found that `split` leaves a UTF-8 BOM inside the first statement,
    /// and `files::read_file` does not strip one. So a `.sql` file saved by
    /// Notepad or Excel — which is exactly where a folder of hand-written
    /// scripts comes from — would send `\u{feff}SELECT …` to MySQL. Whether
    /// that is fatal or merely ugly decides whether the reader must strip it or
    /// may leave it.
    ///
    /// The same question for a statement that leads with its comment, which
    /// §9.1 showed is the normal case rather than the odd one.
    #[tokio::test]
    #[ignore = "needs the live fixture"]
    async fn measure_whether_a_bom_reaches_the_server() {
        println!("\n§9.7 a BOM, and a leading comment, on the wire");
        let mut conn = connect().await;

        for (name, sql) in [
            ("plain", "SELECT 1".to_string()),
            ("leading line comment", "-- hi\nSELECT 1".to_string()),
            ("leading block comment", "/* hi */ SELECT 1".to_string()),
            ("leading BOM", "\u{feff}SELECT 1".to_string()),
            ("BOM then comment", "\u{feff}-- hi\nSELECT 1".to_string()),
        ] {
            let got = sqlx::query(sqlx::AssertSqlSafe(sql.clone()))
                .fetch_one(&mut conn)
                .await
                .map(|r| r.try_get::<i64, _>(0).unwrap());
            match got {
                Ok(v) => println!("  {name:22} -> ok ({v})"),
                Err(e) => println!("  {name:22} -> REFUSED: {e}"),
            }
        }
    }

    /// **§9.4 — CSV throughput**, so the progress line's update rate and any
    /// buffering are chosen rather than guessed. Stage 22 §9.5 found batch size
    /// mattered 250× on load; the equivalent number here is unknown.
    ///
    /// Measures the whole path the runner will use: stream, render with
    /// `export::csv_row`, write to a file. Reported as rows/s and MB/s, and as
    /// how often a 250ms progress tick would fire.
    #[tokio::test]
    #[ignore = "needs the live fixture and export_probe"]
    async fn measure_csv_throughput() {
        use futures_util::StreamExt;

        let opts = db_query_lib::export::CsvOptions {
            delimiter: ",".into(),
            crlf: false,
            bom: false,
            headers: true,
            null_as: String::new(),
            formula_guard: false,
        };

        println!("\n§9.4 CSV throughput, streaming export_probe.wide");
        for flush_every in [1usize, 1_000, 100_000] {
            let mut conn =
                MySqlConnection::connect("mysql://root:devpassword@127.0.0.1:3306/export_probe")
                    .await
                    .unwrap();
            let dir = std::env::temp_dir().join("db-query-csv-measure");
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(format!("wide-{flush_every}.csv"));
            let mut file = std::fs::File::create(&path).unwrap();

            let start = std::time::Instant::now();
            let mut rows = 0u64;
            let mut buf = String::new();
            let mut header = true;
            let mut stream = sqlx::query("SELECT * FROM wide").fetch(&mut conn);
            while let Some(row) = stream.next().await {
                let row = row.unwrap();
                if header {
                    buf.push_str(&db_query_lib::export::csv_header(&row, &opts));
                    header = false;
                }
                buf.push_str(&db_query_lib::export::csv_row(&row, &opts));
                rows += 1;
                if rows % flush_every as u64 == 0 {
                    file.write_all(buf.as_bytes()).unwrap();
                    buf.clear();
                }
            }
            drop(stream);
            file.write_all(buf.as_bytes()).unwrap();
            file.flush().unwrap();
            let secs = start.elapsed().as_secs_f64();
            let bytes = std::fs::metadata(&path).unwrap().len();
            println!(
                "  write every {flush_every:>6} rows: {rows} rows in {secs:.2}s \
                 = {:.0} rows/s, {:.1} MB, {:.1} MB/s",
                rows as f64 / secs,
                bytes as f64 / 1_048_576.0,
                bytes as f64 / 1_048_576.0 / secs,
            );
            std::fs::remove_file(&path).ok();
        }
    }

    /// **§9.5b — the trap that broke §9.5's first attempt, on purpose.**
    ///
    /// Two facts, each on its own fresh connection so the prepare order is
    /// controlled rather than inherited:
    ///
    ///   * a prepared `SELECT DATABASE()` **prepared before** a `USE` keeps
    ///     answering with the old database, because sqlx caches prepared
    ///     statements per connection by SQL text and MySQL resolved
    ///     `DATABASE()` at prepare time;
    ///   * `Executor::execute("USE …")` versus `raw_sql("USE …")` — whether
    ///     sqlx's prepared path can change the session database at all.
    ///
    /// Both decide how the job sets its database (§5) and how the report knows
    /// which one each file ran in (§3.4). Neither can be answered by reading
    /// code.
    #[tokio::test]
    #[ignore = "needs the live fixture and export_probe"]
    async fn measure_the_prepared_statement_cache_and_use() {
        use sqlx::Row as _;

        async fn prepared_db(conn: &mut MySqlConnection) -> Option<String> {
            sqlx::query("SELECT DATABASE()")
                .fetch_one(conn)
                .await
                .unwrap()
                .try_get(0)
                .unwrap()
        }
        async fn text_db(conn: &mut MySqlConnection) -> Option<String> {
            sqlx::raw_sql(sqlx::AssertSqlSafe("SELECT DATABASE()".to_string()))
                .fetch_one(conn)
                .await
                .unwrap()
                .try_get(0)
                .unwrap()
        }

        println!("\n§9.5b the prepared-statement cache, and the two USE spellings");

        // --- the cache. Prepare first, then USE, then ask again.
        let mut conn = connect().await;
        let before = prepared_db(&mut conn).await;
        sqlx::raw_sql(sqlx::AssertSqlSafe("USE `export_probe`".to_string()))
            .execute(&mut conn)
            .await
            .unwrap();
        let after_cached = prepared_db(&mut conn).await;
        let after_text = text_db(&mut conn).await;
        println!("  prepared before USE:        {before:?}");
        println!("  same prepared after USE:    {after_cached:?}");
        println!("  text protocol after USE:    {after_text:?}");
        assert_eq!(after_text.as_deref(), Some("export_probe"));
        assert_eq!(
            after_cached.as_deref(),
            Some("poc"),
            "the cached prepared statement gave a fresh answer, so this trap is \
             gone and §3.4 may ask the server directly"
        );

        // --- the two spellings, each on a connection that has never prepared
        //     `SELECT DATABASE()`, so the cache cannot confuse the answer.
        for spelling in ["execute", "raw_sql"] {
            let mut c = connect().await;
            match spelling {
                "execute" => {
                    c.execute("USE `export_probe`").await.unwrap();
                }
                _ => {
                    sqlx::raw_sql(sqlx::AssertSqlSafe("USE `export_probe`".to_string()))
                        .execute(&mut c)
                        .await
                        .unwrap();
                }
            }
            println!(
                "  {spelling:8} then text DATABASE(): {:?}",
                text_db(&mut c).await
            );
        }
    }

    /// **§9.5 — does a `USE` inside one script reach the next?**
    ///
    /// The job runs every file on one connection, so if `USE` persists then a
    /// script that switches database silently changes where every later file
    /// runs. §3.4 promises the report names the database each file ran against,
    /// and this is the fact that promise rests on.
    ///
    /// **It took two attempts, and the first one lied.** Asking
    /// `SELECT DATABASE()` through sqlx's ordinary path said the database had
    /// *not* changed. It had: `DATABASE()` is resolved when MySQL **prepares**
    /// the statement, sqlx caches prepared statements per connection by their
    /// SQL text, so every later execution replayed the answer from before the
    /// `USE`. The measurement was broken, not the `USE`.
    ///
    /// That trap matters beyond this test — it is why §3.4's per-file database
    /// cannot be implemented by asking the server through a cached statement.
    /// So this measures the **consequence** instead: `t` exists in
    /// `export_probe` and not in `poc`, so an unqualified `FROM t` is a
    /// question only the session's real database can answer.
    #[tokio::test]
    #[ignore = "needs the live fixture and export_probe"]
    async fn measure_whether_use_outlives_a_script() {
        /// Unqualified, so only the session's current database can resolve it.
        async fn unqualified_t(conn: &mut MySqlConnection) -> Result<i64, String> {
            sqlx::query("SELECT COUNT(*) FROM t")
                .fetch_one(conn)
                .await
                .map(|r| r.try_get::<i64, _>(0).unwrap())
                .map_err(|e| e.to_string())
        }

        /// `DATABASE()` over the text protocol, which is resolved now rather
        /// than at prepare time.
        async fn database_now(conn: &mut MySqlConnection) -> Option<String> {
            use sqlx::Row as _;
            sqlx::raw_sql(sqlx::AssertSqlSafe("SELECT DATABASE()".to_string()))
                .fetch_one(conn)
                .await
                .unwrap()
                .try_get::<Option<String>, _>(0)
                .unwrap()
        }

        println!("\n§9.5 does USE persist across statements on one connection?");
        let mut conn = connect().await;

        println!(
            "  on connect, DATABASE():        {:?}",
            database_now(&mut conn).await
        );
        println!(
            "  unqualified `t` in poc:        {:?}",
            unqualified_t(&mut conn).await.err().map(|e| e.len())
        );
        assert!(
            unqualified_t(&mut conn).await.is_err(),
            "`t` must not exist in poc, or this measurement proves nothing"
        );

        // The app's own spelling: `raw_sql`, the text protocol.
        sqlx::raw_sql(sqlx::AssertSqlSafe("USE `export_probe`".to_string()))
            .execute(&mut conn)
            .await
            .unwrap();

        let rows = unqualified_t(&mut conn).await;
        println!("  after USE, unqualified `t`:    {rows:?}");
        println!(
            "  after USE, DATABASE():         {:?}",
            database_now(&mut conn).await
        );
        assert!(
            rows.is_ok(),
            "USE did not take effect, which would leave the job no way to set \
             a database: {rows:?}"
        );

        // "The next file" is just the next statement on the same connection.
        let still = unqualified_t(&mut conn).await;
        println!("  one statement later:           {still:?}");
        assert!(
            still.is_ok(),
            "USE did not persist, so §3.4's per-file database reporting is \
             pointless"
        );

        // **And the trap, measured rather than asserted.** A cached prepared
        // statement keeps answering with the database that was active when it
        // was prepared.
        use sqlx::Row as _;
        let stale: Option<String> = sqlx::query("SELECT DATABASE()")
            .fetch_one(&mut conn)
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
        println!("  prepared DATABASE() says:      {stale:?}  <- resolved at PREPARE");
        println!(
            "  text-protocol DATABASE() says: {:?}",
            database_now(&mut conn).await
        );
    }
}
