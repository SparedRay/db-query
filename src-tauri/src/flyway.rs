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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Disagreement {
    Host,
    Port,
    Database,
    User,
}

impl Disagreement {
    pub fn field(self) -> &'static str {
        match self {
            Disagreement::Host => "host",
            Disagreement::Port => "port",
            Disagreement::Database => "database",
            Disagreement::User => "user",
        }
    }
}

/// Why an apply must not go ahead, or `None`.
///
/// **Extracted from the command so the guards are testable without a running
/// app.** Each of these is the difference between changing the database you
/// are looking at and changing a different one, and none of them should be
/// reachable only through a Tauri handle.
///
/// The order matters. Read-only is a decision the user made and can unmake, so
/// it is said first and on its own terms; a disagreement is a fact about two
/// files that drifted apart, and it names the fields.
pub fn apply_refusal(
    project: &Project,
    profile: &crate::session::ConnProfile,
    environment: &str,
) -> Option<String> {
    if profile.read_only {
        return Some(
            r#"This connection is marked read-only, so it will not apply migrations. Edit the connection and clear "Read-only" to allow writes."#
                .into(),
        );
    }

    let Some(env) = project.environment(environment) else {
        return Some(format!(
            "This project no longer has an environment called \"{environment}\".              It may have been renamed or removed since it was attached."
        ));
    };

    // Checked **again**, not only at import. The TOML lives in a repository
    // and changes with the branch — which is the actual workflow this was
    // built for — so the file that was checked is not necessarily the file
    // about to be used.
    let disagreements = disagreements(env, profile);
    if disagreements.is_empty() {
        return None;
    }
    let fields: Vec<&str> = disagreements.iter().map(|d| d.field()).collect();
    let verb = if fields.len() == 1 {
        "differs"
    } else {
        "differ"
    };
    let fields = fields.join(" and ");
    Some(format!(
        // "user differs" for one and "host and user differ" for two: a
        // guard that cannot manage a plural reads as machine output, and
        // this one is asking somebody to stop and check something.
        r#""{environment}" no longer matches this connection: {fields} {verb}. The project file may have changed since it was attached. Re-attach it from the migrations pane before applying."#
    ))
}

/// Compare an environment with the connection it is about to be attached to.
///
/// **Why this exists.** `flyway migrate -environment=uat` connects to the URL
/// *in the TOML*, not to our connection. Attach the wrong environment and you
/// would be watching one database while changing another — the exact thing
/// this app's colours, labels and read-only flag exist to prevent.
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
    // A profile with no database is pointed at the server, not at one schema,
    // so it cannot disagree with the environment about which schema that is.
    if let (Some(theirs), Some(ours)) = (
        target.database.as_deref(),
        profile.database.as_deref().filter(|d| !d.is_empty()),
    ) {
        if !theirs.eq_ignore_ascii_case(ours) {
            out.push(Disagreement::Database);
        }
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
            flyway_project: None,
            flyway_environment: None,
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
            vec![
                Disagreement::Host,
                Disagreement::Port,
                Disagreement::Database,
                Disagreement::User
            ],
            "the refusal has to say which field disagreed"
        );
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

    // ------------------------------------------------- refusing to apply

    /// The environment the real file's `development` points at.
    fn agreeing() -> crate::session::ConnProfile {
        profile("dev.example.com", 3306, Some("flyway"), "cftconn_dev_app")
    }

    #[test]
    fn an_agreeing_environment_is_not_refused() {
        let p = parse(REAL).unwrap();
        assert_eq!(apply_refusal(&p, &agreeing(), "development"), None);
    }

    /// F7. Said in the connection's own terms — it is a choice the user made
    /// and can unmake — and *before* anything else, because it is true
    /// whatever the project file says.
    #[test]
    fn a_read_only_connection_refuses_to_apply() {
        let p = parse(REAL).unwrap();
        let mut ro = agreeing();
        ro.read_only = true;

        let reason = apply_refusal(&p, &ro, "development").expect("refused");
        assert!(reason.contains("read-only"), "{reason}");
        assert!(
            reason.contains("Edit the connection"),
            "it says how to undo it"
        );

        // And it refuses even when the environment would *also* have been
        // wrong, so the user is not sent to fix the wrong thing first.
        let mut wrong = ro.clone();
        wrong.host = "somewhere.else".into();
        assert!(apply_refusal(&p, &wrong, "development")
            .unwrap()
            .contains("read-only"));
    }

    /// **The case this guard exists for.** The TOML is in a repository and
    /// changes with the branch, so an environment that matched at import can
    /// stop matching without anybody touching the connection.
    #[test]
    fn an_environment_that_has_drifted_since_import_refuses_and_names_the_fields() {
        let p = parse(REAL).unwrap();
        let moved = profile("dev.example.com", 3306, Some("flyway"), "someone_else");

        let reason = apply_refusal(&p, &moved, "development").expect("refused");
        assert!(
            reason.contains("user differs"),
            "it names what differs: {reason}"
        );
        assert!(!reason.contains("host"), "and not what does not: {reason}");
        assert!(reason.contains("Re-attach"), "{reason}");
    }

    #[test]
    fn two_fields_that_differ_are_both_named() {
        let p = parse(REAL).unwrap();
        // A connection pointed at dev, checked against the qa environment:
        // both the server and the account are somebody else's.
        let elsewhere = profile("dev.example.com", 3306, Some("flyway"), "cftconn_dev_app");

        let reason = apply_refusal(&p, &elsewhere, "qa").expect("refused");
        assert!(reason.contains("host and user differ"), "{reason}");
    }

    /// An environment renamed out from under the connection is not a
    /// disagreement — there is nothing to compare — and saying "host differs"
    /// would send somebody looking at the wrong thing.
    #[test]
    fn an_environment_that_no_longer_exists_says_so() {
        let p = parse(REAL).unwrap();
        let reason = apply_refusal(&p, &agreeing(), "staging").expect("refused");
        assert!(reason.contains("staging"), "{reason}");
        assert!(reason.contains("renamed or removed"), "{reason}");
    }
}
