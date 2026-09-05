//! Cost of the scanning we do on every keystroke, at file-sized buffers.
//!
//! Not a pass/fail suite — it prints numbers so the tracker can record them and
//! the lint threshold can be chosen from evidence rather than a guess.
//!
//!     cargo test --release --test perf -- --ignored --nocapture

use std::collections::HashMap;
use std::time::Instant;

use db_query_lib::{lint, split};

/// A buffer that looks like a real dump: many statements, strings, comments.
fn synthetic_sql(target_bytes: usize) -> String {
    let unit = "\
-- seed a row and read it back
INSERT INTO users (email, display_name, balance) VALUES ('a@b.example', 'Ada ''A''', 10.50);
SELECT u.id, u.email, o.total /* inline note */ FROM users u
  JOIN orders o ON o.user_id = u.id
  WHERE u.email LIKE 'a%' AND o.total > 1 ORDER BY o.total DESC;
UPDATE users SET display_name = 'x; not a split' WHERE id = 1;
";
    let mut s = String::with_capacity(target_bytes + unit.len());
    while s.len() < target_bytes {
        s.push_str(unit);
    }
    s
}

fn schema() -> lint::LintSchema {
    let mut m: HashMap<String, Vec<String>> = HashMap::new();
    m.insert(
        "users".into(),
        vec![
            "id".into(),
            "email".into(),
            "display_name".into(),
            "balance".into(),
        ],
    );
    m.insert(
        "orders".into(),
        vec!["id".into(), "user_id".into(), "total".into()],
    );
    m
}

fn ms(f: impl FnOnce()) -> f64 {
    let t = Instant::now();
    f();
    t.elapsed().as_secs_f64() * 1000.0
}

#[test]
#[ignore]
fn measure_scan_costs_across_buffer_sizes() {
    let schema = schema();
    println!(
        "\n{:>10} {:>8} {:>12} {:>12} {:>12}",
        "size", "stmts", "split ms", "mask ms", "lint ms"
    );
    println!("{}", "-".repeat(58));

    for kb in [64usize, 256, 1024, 5 * 1024] {
        let sql = synthetic_sql(kb * 1024);
        let n = split::split(&sql).statements.len();

        let t_split = ms(|| {
            std::hint::black_box(split::split(&sql));
        });
        let t_mask = ms(|| {
            std::hint::black_box(split::mask_noncode(&sql));
        });
        let t_lint = ms(|| {
            std::hint::black_box(lint::lint(&sql, &schema));
        });

        println!(
            "{:>8} KB {:>8} {:>12.1} {:>12.1} {:>12.1}",
            kb, n, t_split, t_mask, t_lint
        );
    }
    println!();
}
