//! Reading a Flyway project's `flyway.toml`.
//!
//! # What this reads, and what it refuses to
//!
//! Four things: the environments, their JDBC URLs and users, which environment
//! the file names as current, and the project's `outOfOrder` default. Nothing
//! else — the real files carry `[flywayDesktop]`, `[redgateCompare]`,
//! `callbackLocations`, `schemaModelLocation`, and at least one
//! `shadowEnvironment` naming an environment the file does not define.
//! **Validating the file would reject a config that works**, so unknown keys
//! are ignored rather than rejected, and that is what keeps this alive across
//! Flyway and Desktop versions.
//!
//! It also does not read `locations`. `flyway info -outputType=json` reports
//! the full path of every migration file, so the folder is Flyway's business,
//! not ours.
//!
//! # The password
//!
//! Flyway environments carry `password` inline. **No type here has a field
//! that could hold one** — the same construction `ConnProfile` uses, and for
//! the same reason: there is then nothing to remember to strip before
//! logging, serialising or showing it. The file's bytes pass through the
//! parser; nothing keeps them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One environment: a database, in Flyway's vocabulary.
///
/// Serialised to the frontend so the import dialog can offer them. It carries
/// no password because **no type in this module has a field for one**.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    /// The table name — `[environments.uat]` is `uat`. What `-environment=`
    /// takes.
    pub id: String,
    /// Flyway Desktop's label, e.g. "UAT database". Shown in preference to the
    /// id where there is room, because it is what the user named it.
    pub display_name: Option<String>,
    pub url: String,
    pub user: Option<String>,
}

impl Environment {
    /// What to show a person: their label, falling back to the id.
    pub fn label(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub name: Option<String>,
    /// `databaseType = "MySql"`. Used to refuse an import into a connection
    /// that speaks something else, with a message that says so.
    pub database_type: Option<String>,
    /// Sorted by id, so the order the user is offered does not depend on how
    /// the file happened to be written.
    pub environments: Vec<Environment>,
    /// `[flyway] environment` — which one the file currently targets. The
    /// default offered at import, because a branch's config already answers
    /// this question.
    pub default_environment: Option<String>,
    /// `[flyway] outOfOrder`. The project's own default; the apply dialog
    /// starts here rather than at a value this app invented.
    pub out_of_order: bool,
}

impl Project {
    pub fn environment(&self, id: &str) -> Option<&Environment> {
        self.environments.iter().find(|e| e.id == id)
    }
}

// ------------------------------------------------------------------ parsing

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawProject {
    name: Option<String>,
    database_type: Option<String>,
    #[serde(default)]
    environments: BTreeMap<String, RawEnvironment>,
    #[serde(default)]
    flyway: RawFlyway,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawEnvironment {
    url: String,
    user: Option<String>,
    display_name: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RawFlyway {
    environment: Option<String>,
    out_of_order: Option<bool>,
}

pub fn parse(text: &str) -> Result<Project, String> {
    let raw: RawProject =
        toml::from_str(text).map_err(|e| format!("This is not a readable Flyway project: {e}"))?;

    let environments = raw
        .environments
        .into_iter()
        .map(|(id, e)| Environment {
            id,
            display_name: e.display_name,
            url: e.url,
            user: e.user,
        })
        .collect::<Vec<_>>();

    if environments.is_empty() {
        return Err(
            "This project defines no environments, so there is nothing to connect to. \
                    A Flyway project needs at least one `[environments.<name>]` table."
                .into(),
        );
    }

    Ok(Project {
        name: raw.name,
        database_type: raw.database_type,
        environments,
        default_environment: raw.flyway.environment,
        out_of_order: raw.flyway.out_of_order.unwrap_or(false),
    })
}

// ------------------------------------------------------------ the JDBC URL

/// The parts of a JDBC URL that say **which database this is**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub database: Option<String>,
}

/// Pull host, port and database out of `jdbc:mysql://host:3306/db?a=1&b=2`.
///
/// Everything after `?` is discarded on purpose: `allowPublicKeyRetrieval`,
/// `useSSL`, `createDatabaseIfNotExist` and friends say how to connect, not
/// what to. Returns `None` rather than guessing when the shape is unfamiliar —
/// a guess here would be a guess about which server is about to be migrated.
pub fn parse_jdbc(url: &str) -> Option<Target> {
    let rest = url.strip_prefix("jdbc:")?;
    // `mysql://`, `mariadb://`, `postgresql://` … the sub-protocol is the
    // engine's business; the shape after `//` is what this reads.
    let after = rest.split_once("://")?.1;
    let authority_and_path = after.split(['?', ';']).next()?;
    let (authority, path) = match authority_and_path.split_once('/') {
        Some((a, p)) => (a, Some(p)),
        None => (authority_and_path, None),
    };
    if authority.is_empty() {
        return None;
    }

    // An IPv6 literal is bracketed, and the colons inside it are not a port.
    let (host, port) = if let Some(end) = authority
        .strip_prefix('[')
        .and_then(|_| authority.find(']'))
    {
        let host = &authority[..=end];
        match authority[end + 1..].strip_prefix(':') {
            Some(p) => (host, Some(p)),
            None => (host, None),
        }
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        }
    };

    let port = match port {
        Some(p) => p.parse::<u16>().ok()?,
        // MySQL's own default, and the same one `ConnProfile` uses when a
        // profile does not say.
        None => 3306,
    };

    let database = path
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(|d| d.to_string());

    Some(Target {
        host: host.to_string(),
        port,
        database,
    })
}

// ------------------------------------------------------------- the guard

/// Which field of a connection an environment disagrees with.
///
/// **Not the database.** It used to be compared, and it refused the first real
/// migration anybody tried: the project's URL ends in `/flyway` because that is
/// the schema holding Flyway's history table, and the connection defaults to
/// `maindatabase` because that is where its user works. In MySQL the database in
/// a URL is only the session's default schema — the same server, the same
/// account, the same reach. Host, port and user say *which server is about to be
/// changed and by whom*; the default schema says neither, and a guard that can
/// never pass on a correct setup teaches people to override it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Disagreement {
    Host,
    Port,
    User,
}

impl Disagreement {
    pub fn field(self) -> &'static str {
        match self {
            Disagreement::Host => "host",
            Disagreement::Port => "port",
            Disagreement::User => "user",
        }
    }
}

/// What is about to be done to an environment. Only the refusal's sentence
/// differs, and it differs in the one word that says what was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Write {
    Apply,
    Repair,
}

impl Write {
    fn phrase(self) -> &'static str {
        match self {
            Write::Apply => "apply migrations",
            Write::Repair => "repair the schema history",
        }
    }
}

/// A saved connection that an environment points at.
///
/// **Computed, never stored** (Stage 17 §3.2). The file changes with the
/// branch, so a remembered match would be a claim about whatever was checked
/// out when it was made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Match {
    pub connection_id: String,
    pub name: String,
    pub colour: String,
    pub read_only: bool,
}

/// Every saved connection this environment is — same host, port and user.
///
/// Only MySQL profiles are candidates: an Elasticsearch profile has no host in
/// the sense a JDBC URL means, and a match against one would be a coincidence.
/// In the order the rail shows them, so "the first match" is the one the user
/// would find first.
pub fn matches(env: &Environment, profiles: &[crate::session::ConnProfile]) -> Vec<Match> {
    profiles
        .iter()
        .filter(|p| p.kind == crate::session::EngineKind::Mysql)
        .filter(|p| disagreements(env, p).is_empty())
        .map(|p| Match {
            connection_id: p.id.clone(),
            name: p.name.clone(),
            colour: p.colour.clone(),
            read_only: p.read_only,
        })
        .collect()
}

/// What the user was shown, and agreed to, in the confirmation.
///
/// Handed back to the command so it can refuse if the file now says something
/// else. The dialog is built from the file as it read when the dialog opened; a
/// branch switched before the button was pressed would otherwise make the
/// confirmation a description of a different database.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Confirmed {
    pub url: String,
    pub user: Option<String>,
}

/// Why a write must not go ahead, or `None`.
///
/// Shared by apply and repair. Repair writes less than apply does — one table
/// rather than a schema — but to the same database, so it is asked the same
/// questions.
///
/// Three refusals, in this order:
///
///   1. **The environment is gone** — renamed or removed on this branch.
///   2. **The target moved** since the confirmation was shown.
///   3. **A matching connection is read-only.** If *any* is, this is: two
///      profiles for the same server and account that disagree about whether
///      writes are allowed get the safe answer, not the convenient one.
///
/// An environment that matches no saved connection is **not** refused. That is
/// often the migration account, which people do not browse with, and refusing
/// it would rebuild the dead end Stage 16 §17.2 removed. The confirmation says
/// plainly that the target is not one of your connections.
pub fn write_refusal(
    project: &Project,
    environment: &str,
    confirmed: &Confirmed,
    profiles: &[crate::session::ConnProfile],
    what: Write,
) -> Option<String> {
    let Some(env) = project.environment(environment) else {
        return Some(format!(
            "This project no longer has an environment called \"{environment}\". \
             It may have been renamed or removed on this branch."
        ));
    };

    if env.url != confirmed.url || env.user != confirmed.user {
        return Some(format!(
            "\"{environment}\" changed in the project file after you confirmed \
             \u{2014} it now points at {}{}. Nothing was run. Refresh and check the \
             target again.",
            describe_target(env),
            env.user
                .as_deref()
                .map(|u| format!(" as {u}"))
                .unwrap_or_default(),
        ));
    }

    if let Some(ro) = matches(env, profiles).into_iter().find(|m| m.read_only) {
        return Some(format!(
            r#""{environment}" is your connection "{}", which is marked read-only, so this will not {}. Edit that connection and clear "Read-only" to allow writes."#,
            ro.name,
            what.phrase()
        ));
    }
    None
}

/// `host:port`, or the raw URL when it cannot be read — never a guess.
pub fn describe_target(env: &Environment) -> String {
    match parse_jdbc(&env.url) {
        Some(t) => format!("{}:{}", t.host, t.port),
        None => env.url.clone(),
    }
}

/// One environment as the pane shows it: where it points and what it is.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentView {
    pub id: String,
    pub display_name: Option<String>,
    /// Echoed back in [`Confirmed`]. Carries no password — `Environment` has
    /// no field for one.
    pub url: String,
    pub user: Option<String>,
    /// `host:port`, or the URL when it could not be read.
    pub target: String,
    pub matches: Vec<Match>,
}

pub fn environment_views(
    project: &Project,
    profiles: &[crate::session::ConnProfile],
) -> Vec<EnvironmentView> {
    project
        .environments
        .iter()
        .map(|e| EnvironmentView {
            id: e.id.clone(),
            display_name: e.display_name.clone(),
            url: e.url.clone(),
            user: e.user.clone(),
            target: describe_target(e),
            matches: matches(e, profiles),
        })
        .collect()
}

/// Compare an environment with a saved connection.
///
/// **What it is for now.** `flyway migrate -environment=uat` connects to the
/// URL *in the TOML*, not to any connection of ours. This decides which of the
/// user's connections that URL *is* — [`matches`] — so the pane can name it,
/// colour it, and honour its read-only flag.
///
/// **The user is compared, not just the address.** Environments commonly share
/// a server and differ only by account; comparing host and port alone would
/// wave that case straight through, which is the case most likely to be wrong.
///
/// An empty list means they agree. A URL this cannot parse counts as a
/// disagreement about the host rather than a pass — not understanding the
/// target is not the same as the target being right.
pub fn disagreements(
    env: &Environment,
    profile: &crate::session::ConnProfile,
) -> Vec<Disagreement> {
    let Some(target) = parse_jdbc(&env.url) else {
        return vec![Disagreement::Host];
    };
    let mut out = Vec::new();

    if !same_host(&target.host, &profile.host) {
        out.push(Disagreement::Host);
    }
    if target.port != profile.port {
        out.push(Disagreement::Port);
    }
    if let Some(theirs) = env.user.as_deref().filter(|u| !u.is_empty()) {
        if !theirs.eq_ignore_ascii_case(&profile.user) {
            out.push(Disagreement::User);
        }
    }
    out
}

/// Case-insensitive, and `localhost` and `127.0.0.1` are the same machine.
///
/// Not a general resolver: the point is to stop a *wrong* attachment, and
/// asking DNS whether two names happen to resolve alike would make the answer
/// depend on the network at the moment somebody clicked.
fn same_host(a: &str, b: &str) -> bool {
    let norm = |h: &str| {
        let h = h.trim().trim_start_matches('[').trim_end_matches(']');
        match h.to_ascii_lowercase().as_str() {
            "localhost" | "::1" => "127.0.0.1".to_string(),
            other => other.to_string(),
        }
    };
    norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The user's own project file**, redacted only where a password was.
    /// Every awkward thing in this test is in the real file: environments that
    /// differ *only* by user, tables we do not read, and a `shadowEnvironment`
    /// naming an environment the file never defines.
    const REAL: &str = r#"
databaseType = "MySql"
id = "24b450ed-46c4-44c4-a560-e7fdfe3a2152"
name = "Flyway Connections"

[environments.development]
displayName = "Development database"
password = "hunter2"
url = "jdbc:mysql://dev.example.com:3306/flyway?allowPublicKeyRetrieval=true&trustServerCertificate=true&createDatabaseIfNotExist=true&permitMysqlScheme=true"
user = "cftconn_dev_app"

[environments.qa]
displayName = "QA database"
password = "hunter2"
url = "jdbc:mysql://qa.example.com:3306/flyway?allowPublicKeyRetrieval=true&permitMysqlScheme=true"
user = "cftconn_qa_app"

[environments.uat]
displayName = "UAT database"
password = "hunter2"
url = "jdbc:mysql://uat.example.com:3306/flyway?allowPublicKeyRetrieval=true&permitMysqlScheme=true"
user = "cftconn_uat_app"

[flyway]
callbackLocations = [ "filesystem:callbacks" ]
defaultSchema = "flyway"
environment = "uat"
locations = [ "filesystem:../myproject/src/Flyway/MasterScripts" ]
mixed = true
outOfOrder = false
schemaModelLocation = "schema-model"
validateMigrationNaming = true

[flywayDesktop]
enableMigrations = true
developmentEnvironment = "development"
shadowEnvironment = "shadow"

[flywayDesktop.generate]
undoScripts = false

[redgateCompare]
filterFile = "filter.rgf"

[redgateCompare.mysql.options.ignores]
ignoreNewlinesInTextObjects = "off"
"#;

    fn profile(host: &str, port: u16, db: Option<&str>, user: &str) -> crate::session::ConnProfile {
        crate::session::ConnProfile {
            id: "p".into(),
            name: "n".into(),
            colour: "#3b82f6".into(),
            host: host.into(),
            port,
            user: user.into(),
            database: db.map(str::to_string),
            allow_invalid_certs: false,
            kind: Default::default(),
            url: String::new(),
            auth: Default::default(),
            no_password: false,
            read_only: false,
        }
    }

    #[test]
    fn the_real_project_file_parses() {
        let p = parse(REAL).expect("the file the user actually uses");
        assert_eq!(p.name.as_deref(), Some("Flyway Connections"));
        assert_eq!(p.database_type.as_deref(), Some("MySql"));
        assert_eq!(p.default_environment.as_deref(), Some("uat"));
        assert!(!p.out_of_order, "the project's own default, not ours");

        let ids: Vec<&str> = p.environments.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["development", "qa", "uat"], "sorted, not file order");
        assert_eq!(p.environment("uat").unwrap().label(), "UAT database");
        assert_eq!(
            p.environment("qa").unwrap().user.as_deref(),
            Some("cftconn_qa_app")
        );
    }

    /// **The property, not the filter.** No type here has a field a password
    /// could land in, so one cannot be logged, serialised or shown by
    /// accident. Asserted on the debug output because that is the form these
    /// values reach an error message in.
    #[test]
    fn a_password_in_the_file_reaches_nothing_we_keep() {
        let p = parse(REAL).unwrap();
        let dumped = format!("{p:?}");
        assert!(
            !dumped.contains("hunter2"),
            "a secret from the project file survived parsing: {dumped}"
        );
    }

    /// Tables and keys we do not read must not stop the file being read. The
    /// real one has four of them, and a `shadowEnvironment` pointing at an
    /// environment that does not exist.
    #[test]
    fn what_we_do_not_read_cannot_break_us() {
        let p = parse(REAL).unwrap();
        assert_eq!(p.environments.len(), 3, "shadow is not an environment here");
    }

    #[test]
    fn a_project_with_no_environments_says_so() {
        let e = parse("name = \"empty\"\n[flyway]\nenvironment = \"nope\"\n").unwrap_err();
        assert!(e.contains("no environments"), "{e}");
    }

    #[test]
    fn nonsense_is_refused_rather_than_guessed_at() {
        assert!(parse("this is not toml {{{").is_err());
    }

    // ------------------------------------------------------------ JDBC URLs

    #[test]
    fn a_jdbc_url_gives_up_its_host_port_and_database() {
        let t = parse_jdbc(
            "jdbc:mysql://db.example.com:3307/flyway?allowPublicKeyRetrieval=true&useSSL=false",
        )
        .expect("a normal MySQL JDBC URL");
        assert_eq!(t.host, "db.example.com");
        assert_eq!(t.port, 3307);
        assert_eq!(t.database.as_deref(), Some("flyway"));
    }

    #[test]
    fn a_url_without_a_port_means_the_default() {
        let t = parse_jdbc("jdbc:mysql://db.example.com/flyway").unwrap();
        assert_eq!(t.port, 3306);
        assert_eq!(t.database.as_deref(), Some("flyway"));
    }

    #[test]
    fn a_url_with_no_database_names_none() {
        let t = parse_jdbc("jdbc:mysql://db.example.com:3306").unwrap();
        assert_eq!(t.database, None);
        let t = parse_jdbc("jdbc:mysql://db.example.com:3306/").unwrap();
        assert_eq!(t.database, None);
    }

    /// The colons in an IPv6 literal are not a port, and this is the mistake
    /// `mcp::origin_is_local` already made once.
    #[test]
    fn an_ipv6_literal_keeps_its_colons() {
        let t = parse_jdbc("jdbc:mysql://[::1]:3306/flyway").unwrap();
        assert_eq!(t.host, "[::1]");
        assert_eq!(t.port, 3306);
    }

    #[test]
    fn something_that_is_not_a_jdbc_url_is_not_guessed_at() {
        assert!(parse_jdbc("mysql://db.example.com/flyway").is_none());
        assert!(parse_jdbc("jdbc:mysql:relative/path").is_none());
        assert!(parse_jdbc("").is_none());
        // A port that is not a number is not a port.
        assert!(parse_jdbc("jdbc:mysql://host:abc/db").is_none());
    }

    // -------------------------------------------------------------- the guard

    #[test]
    fn an_environment_that_matches_disagrees_about_nothing() {
        let p = parse(REAL).unwrap();
        let env = p.environment("uat").unwrap();
        let conn = profile("uat.example.com", 3306, Some("flyway"), "cftconn_uat_app");
        assert_eq!(disagreements(env, &conn), vec![]);
    }

    /// **The case the guard exists for.** Environments commonly share a server
    /// and differ only by account; comparing host and port alone would wave
    /// the wrong environment straight through.
    #[test]
    fn the_same_server_with_a_different_account_is_a_disagreement() {
        let p = parse(REAL).unwrap();
        let env = p.environment("uat").unwrap();
        let conn = profile("uat.example.com", 3306, Some("flyway"), "cftconn_dev_app");
        assert_eq!(disagreements(env, &conn), vec![Disagreement::User]);
    }

    #[test]
    fn a_different_server_is_named_field_by_field() {
        let p = parse(REAL).unwrap();
        let env = p.environment("uat").unwrap();
        let conn = profile("prod.example.com", 3307, Some("other"), "someone");
        let d = disagreements(env, &conn);
        assert_eq!(
            d,
            vec![Disagreement::Host, Disagreement::Port, Disagreement::User],
            "the refusal has to say which field disagreed"
        );
    }

    /// **The first real migration.** The project's URL names Flyway's history
    /// schema; the connection names the schema its user works in. Same server,
    /// same account — the same database, and it has to match.
    #[test]
    fn a_different_default_schema_is_not_a_different_target() {
        let p = parse(REAL).unwrap();
        let env = p.environment("qa").unwrap();
        let conn = profile(
            "qa.example.com",
            3306,
            Some("maindatabase"),
            "cftconn_qa_app",
        );
        assert_eq!(disagreements(env, &conn), vec![]);
        assert_eq!(matches(env, &[conn]).len(), 1);
    }

    /// A connection pointed at the server rather than at one schema cannot
    /// disagree about which schema it is.
    #[test]
    fn a_connection_with_no_database_does_not_disagree_about_one() {
        let p = parse(REAL).unwrap();
        let env = p.environment("uat").unwrap();
        let conn = profile("uat.example.com", 3306, None, "cftconn_uat_app");
        assert_eq!(disagreements(env, &conn), vec![]);
    }

    /// Not understanding the target is not the same as the target being right.
    #[test]
    fn an_unreadable_url_is_a_disagreement_not_a_pass() {
        let env = Environment {
            id: "odd".into(),
            display_name: None,
            url: "definitely not a jdbc url".into(),
            user: None,
        };
        let conn = profile("anywhere", 3306, None, "anyone");
        assert_eq!(disagreements(&env, &conn), vec![Disagreement::Host]);
    }

    #[test]
    fn localhost_and_the_loopback_address_are_the_same_machine() {
        let env = Environment {
            id: "local".into(),
            display_name: None,
            url: "jdbc:mysql://localhost:3306/flyway_dev".into(),
            user: Some("root".into()),
        };
        let conn = profile("127.0.0.1", 3306, Some("flyway_dev"), "root");
        assert_eq!(disagreements(&env, &conn), vec![]);
    }

    // ------------------------------------------------------------ matching

    fn named(mut p: crate::session::ConnProfile, id: &str) -> crate::session::ConnProfile {
        p.id = id.into();
        p.name = id.into();
        p
    }

    fn qa() -> crate::session::ConnProfile {
        named(
            profile(
                "qa.example.com",
                3306,
                Some("maindatabase"),
                "cftconn_qa_app",
            ),
            "QA",
        )
    }

    fn confirmed(p: &Project, env: &str) -> Confirmed {
        let e = p.environment(env).unwrap();
        Confirmed {
            url: e.url.clone(),
            user: e.user.clone(),
        }
    }

    /// F2. Only the connections that are this environment, in rail order.
    #[test]
    fn an_environment_matches_the_connections_that_are_it() {
        let p = parse(REAL).unwrap();
        let env = p.environment("qa").unwrap();
        let other = named(
            profile("dev.example.com", 3306, None, "cftconn_dev_app"),
            "Dev",
        );
        let found = matches(env, &[other, qa()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "QA");
    }

    /// An Elasticsearch profile is never a Flyway target, whatever its host.
    #[test]
    fn only_mysql_connections_are_candidates() {
        let p = parse(REAL).unwrap();
        let env = p.environment("qa").unwrap();
        let mut es = qa();
        es.kind = crate::session::EngineKind::Elasticsearch;
        assert!(matches(env, &[es]).is_empty());
    }

    // ------------------------------------------------- refusing to write

    #[test]
    fn a_writable_match_is_not_refused() {
        let p = parse(REAL).unwrap();
        assert_eq!(
            write_refusal(&p, "qa", &confirmed(&p, "qa"), &[qa()], Write::Apply),
            None
        );
    }

    /// F3. Stage 14's flag keeps its meaning: marking the connection read-only
    /// stops this app writing to that database, migrations included. Said in
    /// the connection's own name, so the user knows which one to edit.
    #[test]
    fn a_read_only_match_refuses_both_apply_and_repair() {
        let p = parse(REAL).unwrap();
        let mut ro = qa();
        ro.read_only = true;

        let reason = write_refusal(&p, "qa", &confirmed(&p, "qa"), &[ro.clone()], Write::Apply)
            .expect("refused");
        assert!(reason.contains("\"QA\""), "{reason}");
        assert!(reason.contains("read-only"), "{reason}");
        assert!(reason.contains("apply migrations"), "{reason}");

        let repairing =
            write_refusal(&p, "qa", &confirmed(&p, "qa"), &[ro], Write::Repair).expect("refused");
        assert!(
            repairing.contains("repair the schema history"),
            "{repairing}"
        );
    }

    /// F4. Two profiles for one database that disagree about writes get the
    /// safe answer.
    #[test]
    fn one_read_only_match_among_several_refuses() {
        let p = parse(REAL).unwrap();
        let writable = qa();
        let mut ro = named(qa(), "QA (browse)");
        ro.read_only = true;
        let reason = write_refusal(
            &p,
            "qa",
            &confirmed(&p, "qa"),
            &[writable, ro],
            Write::Apply,
        )
        .expect("refused");
        assert!(reason.contains("QA (browse)"), "{reason}");
    }

    /// Unmatched is unlabelled, not forbidden — often the migration account.
    #[test]
    fn an_environment_matching_nothing_is_not_refused() {
        let p = parse(REAL).unwrap();
        assert_eq!(
            write_refusal(&p, "uat", &confirmed(&p, "uat"), &[qa()], Write::Apply),
            None
        );
    }

    /// F5. The confirmation described one database; the file now names
    /// another. Nothing runs.
    #[test]
    fn a_target_that_moved_after_confirming_is_refused() {
        let p = parse(REAL).unwrap();
        let shown = confirmed(&p, "development");

        let moved = parse(&REAL.replace("dev.example.com", "prod.example.com")).unwrap();
        let reason =
            write_refusal(&moved, "development", &shown, &[], Write::Apply).expect("refused");
        assert!(reason.contains("prod.example.com:3306"), "{reason}");
        assert!(reason.contains("Nothing was run"), "{reason}");

        // The account changing is the same thing.
        let reuser = parse(&REAL.replace("cftconn_dev_app", "root")).unwrap();
        assert!(write_refusal(&reuser, "development", &shown, &[], Write::Apply).is_some());
    }

    /// An environment renamed out from under the selection says so, rather
    /// than reporting some other field as wrong.
    #[test]
    fn an_environment_that_no_longer_exists_says_so() {
        let p = parse(REAL).unwrap();
        let reason =
            write_refusal(&p, "staging", &confirmed(&p, "qa"), &[], Write::Apply).expect("refused");
        assert!(reason.contains("staging"), "{reason}");
        assert!(reason.contains("renamed or removed"), "{reason}");
    }

    /// What the pane is sent: every environment, its target, and its matches.
    #[test]
    fn the_pane_gets_every_environment_with_its_target() {
        let p = parse(REAL).unwrap();
        let views = environment_views(&p, &[qa()]);
        assert_eq!(views.len(), 3);
        let qa_view = views.iter().find(|v| v.id == "qa").unwrap();
        assert_eq!(qa_view.target, "qa.example.com:3306");
        assert_eq!(qa_view.matches.len(), 1);
        assert!(views
            .iter()
            .find(|v| v.id == "uat")
            .unwrap()
            .matches
            .is_empty());
    }
}
