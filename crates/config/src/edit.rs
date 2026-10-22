//! Editing single objects in object files with `toml_edit`, keeping
//! comments, formatting and the order of everything else.

use crate::files::{self, Change};
use crate::{Diagnostic, Origin, Severity, load::RawFile};
use serde::{Deserialize, Serialize};
use std::path::Path;
use toml::{Table, Value};
use toml_edit::{DocumentMut, Item, TableLike};

/// Object types that can be edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Check,
    Role,
    Host,
    HostGroup,
    ServiceGroup,
    Contact,
    ContactGroup,
    Notify,
    Sink,
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::Check,
        Kind::Role,
        Kind::Host,
        Kind::HostGroup,
        Kind::ServiceGroup,
        Kind::Contact,
        Kind::ContactGroup,
        Kind::Notify,
        Kind::Sink,
    ];

    /// The TOML key: `[[host]]`, `[role.x]`, ...
    pub fn key(self) -> &'static str {
        match self {
            Kind::Check => "check",
            Kind::Role => "role",
            Kind::Host => "host",
            Kind::HostGroup => "hostgroup",
            Kind::ServiceGroup => "servicegroup",
            Kind::Contact => "contact",
            Kind::ContactGroup => "contactgroup",
            Kind::Notify => "notify",
            Kind::Sink => "sink",
        }
    }

    /// Named by their key (`[role.linux]`); hosts, notify rules and sinks
    /// by a `name` field in an array of tables.
    fn keyed(self) -> bool {
        !matches!(self, Kind::Host | Kind::Notify | Kind::Sink)
    }

    /// Where objects created in the UI go.
    pub fn ui_file(self) -> &'static str {
        match self {
            Kind::Check => "ui/checks.toml",
            Kind::Role => "ui/roles.toml",
            Kind::Host => "ui/hosts.toml",
            Kind::HostGroup | Kind::ServiceGroup | Kind::ContactGroup => "ui/groups.toml",
            Kind::Contact => "ui/contacts.toml",
            Kind::Notify => "ui/notify.toml",
            Kind::Sink => "ui/sinks.toml",
        }
    }

    /// Field order for new objects; other fields follow alphabetically.
    fn field_order(self) -> &'static [&'static str] {
        match self {
            Kind::Check => &[
                "title",
                "command",
                "shell",
                "passive",
                "format",
                "vars",
                "thresholds",
                "interval",
                "retry_interval",
                "max_attempts",
                "timeout",
                "freshness",
            ],
            Kind::Role => &[
                "title",
                "include",
                "hosts",
                "host_check",
                "graph",
                "node",
                "groups",
                "vars",
                "interval",
                "retry_interval",
                "max_attempts",
                "timeout",
                "service",
            ],
            Kind::Host => &[
                "name",
                "address",
                "roles",
                "tags",
                "groups",
                "node",
                "host_check",
                "graph",
                "vars",
                "enabled",
                "service",
            ],
            Kind::HostGroup | Kind::ServiceGroup | Kind::ContactGroup => &["title", "members"],
            Kind::Contact => &["email", "webhook", "groups"],
            Kind::Notify => &[
                "name",
                "contacts",
                "contact_groups",
                "hosts",
                "services",
                "states",
                "on",
                "enabled",
            ],
            Kind::Sink => &["name", "type", "url", "password_file", "flush", "buffer"],
        }
    }
}

/// An object as written in its file, before templates are applied.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Object {
    pub kind: Kind,
    pub name: String,
    /// Relative to the object directory.
    pub path: String,
    pub line: usize,
    /// The keys as written, including `name` (for templates and contacts,
    /// the key).
    pub fields: Table,
}

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("{0} {1:?} not found")]
    NotFound(&'static str, String),
    #[error("{0} {1:?} already exists in {2}")]
    Exists(&'static str, String, String),
    #[error("{0} {1:?} is written inline in {2}; edit the file instead")]
    Inline(&'static str, String, String),
    #[error("name is required")]
    NoName,
    #[error("{path}: {message}")]
    Parse { path: String, message: String },
    #[error(transparent)]
    File(#[from] files::Error),
}

/// All objects in the object files. Files that don't parse are reported
/// instead.
pub fn objects(dir: &Path) -> std::io::Result<(Vec<Object>, Vec<Diagnostic>)> {
    let mut objects = Vec::new();
    let mut problems = Vec::new();
    for file in files::list(dir)? {
        match parse(&file.path, &file.text) {
            Ok(found) => objects.extend(found),
            Err((line, message)) => problems.push(Diagnostic {
                origin: Some(Origin {
                    file: file.path.into(),
                    line,
                }),
                severity: Severity::Error,
                message,
            }),
        }
    }
    Ok((objects, problems))
}

fn parse(path: &str, text: &str) -> Result<Vec<Object>, (usize, String)> {
    let line = |offset: usize| text[..offset.min(text.len())].matches('\n').count() + 1;
    let file: RawFile = crate::load::parse_file(text).map_err(|e| {
        (
            e.span().map_or(0, |s| line(s.start)),
            e.message().to_string(),
        )
    })?;
    let mut objects = Vec::new();
    let mut push = |kind, name: Option<String>, spanned: toml::Spanned<Table>| {
        let line = line(spanned.span().start);
        let mut fields = spanned.into_inner();
        let name = match name {
            Some(name) => {
                fields.insert("name".into(), Value::String(name.clone()));
                name
            }
            None => fields
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        };
        objects.push(Object {
            kind,
            name,
            path: path.to_string(),
            line,
            fields,
        });
    };
    let keyed = [
        (Kind::Check, file.check),
        (Kind::Role, file.role),
        (Kind::HostGroup, file.hostgroup),
        (Kind::ServiceGroup, file.servicegroup),
        (Kind::Contact, file.contact),
        (Kind::ContactGroup, file.contactgroup),
    ];
    for (kind, map) in keyed {
        for (name, t) in map {
            push(kind, Some(name), t);
        }
    }
    for (kind, list) in [
        (Kind::Host, file.host),
        (Kind::Notify, file.notify),
        (Kind::Sink, file.sink),
    ] {
        for t in list {
            push(kind, None, t);
        }
    }
    objects.sort_by_key(|o| o.line);
    Ok(objects)
}

/// The file change that creates (`name` is `None`), updates or deletes
/// (`fields` is `None`) an object. Existing objects are edited where they
/// are; new ones go to [`Kind::ui_file`]. Nothing is written: pass the
/// change to [`files::preview`] or [`files::apply`].
pub fn edit(
    dir: &Path,
    kind: Kind,
    name: Option<&str>,
    fields: Option<&Table>,
) -> Result<Change, EditError> {
    let path = match name {
        None => kind.ui_file().to_string(),
        Some(name) => {
            let (objects, _) = objects(dir).map_err(|source| files::Error::Io {
                path: String::new(),
                source,
            })?;
            objects
                .into_iter()
                .find(|o| o.kind == kind && o.name == name)
                .ok_or_else(|| EditError::NotFound(kind.key(), name.into()))?
                .path
        }
    };
    let current = files::read(dir, &path)?;
    let text = current.as_ref().map_or("", |f| f.text.as_str());
    let new = edit_text(text, kind, name, fields).map_err(|err| match err {
        EditError::Exists(k, n, _) => EditError::Exists(k, n, path.clone()),
        EditError::Inline(k, n, _) => EditError::Inline(k, n, path.clone()),
        EditError::Parse { message, .. } => EditError::Parse {
            path: path.clone(),
            message,
        },
        other => other,
    })?;
    Ok(Change {
        path,
        base: current.map(|f| f.hash),
        text: Some(new),
    })
}

/// [`edit`] on the text of one file.
pub fn edit_text(
    text: &str,
    kind: Kind,
    name: Option<&str>,
    fields: Option<&Table>,
) -> Result<String, EditError> {
    let mut doc: DocumentMut =
        text.parse()
            .map_err(|e: toml_edit::TomlError| EditError::Parse {
                path: String::new(),
                message: e.message().to_string(),
            })?;
    let key = kind.key();
    let new_name = match fields {
        Some(f) => Some(
            f.get("name")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())
                .ok_or(EditError::NoName)?,
        ),
        None => None,
    };
    let not_found = || EditError::NotFound(key, name.unwrap_or_default().into());
    let inline = || EditError::Inline(key, name.unwrap_or_default().into(), String::new());

    if kind.keyed() {
        // `[template.x]`: the name is the key, not a field.
        let body = fields.map(|f| {
            let mut f = f.clone();
            f.remove("name");
            f
        });
        if doc.get(key).is_none() {
            if name.is_some() {
                return Err(not_found());
            }
            let mut parent = toml_edit::Table::new();
            parent.set_implicit(true);
            doc.insert(key, Item::Table(parent));
        }
        let parent = doc
            .get_mut(key)
            .and_then(Item::as_table_like_mut)
            .ok_or_else(inline)?;
        let item = match name {
            Some(name) => Some(parent.remove(name).ok_or_else(not_found)?),
            None => None,
        };
        if let (Some(body), Some(new_name)) = (body, new_name) {
            if parent.contains_key(new_name) {
                return Err(EditError::Exists(key, new_name.into(), String::new()));
            }
            let item = match item {
                Some(mut item) => {
                    let table = item.as_table_like_mut().ok_or_else(inline)?;
                    update_table(table, &body);
                    item
                }
                None => Item::Table(new_table(kind, &body)),
            };
            parent.insert(new_name, item);
        }
    } else {
        let position = match doc.get(key) {
            None => None,
            Some(Item::ArrayOfTables(list)) => name.and_then(|name| {
                list.iter()
                    .position(|t| t.get("name").and_then(Item::as_str) == Some(name))
            }),
            Some(_) => return Err(inline()),
        };
        match (name, position, fields) {
            (Some(_), None, _) => return Err(not_found()),
            (Some(_), Some(i), None) => {
                if let Some(Item::ArrayOfTables(list)) = doc.get_mut(key) {
                    list.remove(i);
                }
            }
            (Some(_), Some(i), Some(fields)) => {
                if let Some(Item::ArrayOfTables(list)) = doc.get_mut(key)
                    && let Some(table) = list.get_mut(i)
                {
                    update_table(table, fields);
                }
            }
            (None, _, Some(fields)) => {
                let list = doc
                    .entry(key)
                    .or_insert(Item::ArrayOfTables(Default::default()));
                if let Item::ArrayOfTables(list) = list {
                    list.push(new_table(kind, fields));
                }
            }
            (None, _, None) => return Err(EditError::NoName),
        }
    }
    Ok(doc.to_string())
}

/// A new object's table, with its fields in the usual order.
fn new_table(kind: Kind, fields: &Table) -> toml_edit::Table {
    let order = kind.field_order();
    let mut keys: Vec<&String> = fields.keys().collect();
    keys.sort_by_key(|k| (order.iter().position(|o| o == k).unwrap_or(order.len()), *k));
    let mut table = toml_edit::Table::new();
    for key in keys {
        table.insert(key, Item::Value(to_edit(&fields[key])));
    }
    table
}

/// Makes `table` hold exactly `fields`. Unchanged values keep their
/// formatting and comments, changed ones their comments.
fn update_table(table: &mut dyn TableLike, fields: &Table) {
    let gone: Vec<String> = table
        .iter()
        .map(|(k, _)| k.to_string())
        .filter(|k| !fields.contains_key(k))
        .collect();
    for key in gone {
        table.remove(&key);
    }
    for (key, value) in fields {
        match table.get_mut(key) {
            Some(item) => update_item(item, value),
            None => {
                table.insert(key, Item::Value(to_edit(value)));
            }
        }
    }
}

fn update_item(item: &mut Item, value: &Value) {
    if let (Some(table), Value::Table(fields)) = (item.as_table_like_mut(), value) {
        update_table(table, fields);
        return;
    }
    if let Some(old) = item.as_value() {
        if to_toml(old) == *value {
            return;
        }
        let decor = old.decor().clone();
        let mut new = to_edit(value);
        *new.decor_mut() = decor;
        *item = Item::Value(new);
        return;
    }
    *item = Item::Value(to_edit(value));
}

fn to_toml(value: &toml_edit::Value) -> Value {
    use toml_edit::Value as E;
    match value {
        E::String(s) => Value::String(s.value().clone()),
        E::Integer(i) => Value::Integer(*i.value()),
        E::Float(f) => Value::Float(*f.value()),
        E::Boolean(b) => Value::Boolean(*b.value()),
        E::Datetime(d) => {
            let text = d.value().to_string();
            text.parse::<toml::value::Datetime>()
                .map_or(Value::String(text), Value::Datetime)
        }
        E::Array(items) => Value::Array(items.iter().map(to_toml).collect()),
        E::InlineTable(t) => {
            Value::Table(t.iter().map(|(k, v)| (k.to_string(), to_toml(v))).collect())
        }
    }
}

fn to_edit(value: &Value) -> toml_edit::Value {
    match value {
        Value::String(s) => s.as_str().into(),
        Value::Integer(i) => (*i).into(),
        Value::Float(f) => (*f).into(),
        Value::Boolean(b) => (*b).into(),
        Value::Datetime(d) => {
            let text = d.to_string();
            text.parse::<toml_edit::Datetime>()
                .map_or_else(|_| text.as_str().into(), toml_edit::Value::from)
        }
        Value::Array(items) => {
            let array: toml_edit::Array = items.iter().map(to_edit).collect();
            array.into()
        }
        Value::Table(t) => {
            let table: toml_edit::InlineTable =
                t.iter().map(|(k, v)| (k.as_str(), to_edit(v))).collect();
            table.into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(text: &str) -> Table {
        toml::from_str(text).unwrap()
    }

    const FILE: &str = r#"# Web servers
[[host]]
name = "web01"
address = "10.0.0.5"   # primary
tags = ["linux", "web"]

# Keep this one
[[host]]
name = "web02"

[role.linux]
interval = "1m"
max_attempts = 3

[contact.oncall]
email = ["ops@example.com"]
"#;

    #[test]
    fn update_keeps_comments_and_untouched_objects() {
        let new = fields(
            r#"name = "web01"
address = "10.0.0.6"
tags = ["linux", "web"]
node = "site-a""#,
        );
        let out = edit_text(FILE, Kind::Host, Some("web01"), Some(&new)).unwrap();
        assert_eq!(
            out,
            FILE.replace(
                "address = \"10.0.0.5\"   # primary\ntags = [\"linux\", \"web\"]\n",
                "address = \"10.0.0.6\"   # primary\ntags = [\"linux\", \"web\"]\nnode = \"site-a\"\n"
            )
        );
    }

    #[test]
    fn removed_fields_disappear() {
        let new = fields(r#"name = "web01""#);
        let out = edit_text(FILE, Kind::Host, Some("web01"), Some(&new)).unwrap();
        assert!(out.contains("[[host]]\nname = \"web01\"\n\n# Keep this one"));
    }

    #[test]
    fn create_appends_in_field_order() {
        let new = fields(
            r#"tags = ["db"]
address = "10.0.0.9"
name = "db01""#,
        );
        let out = edit_text(FILE, Kind::Host, None, Some(&new)).unwrap();
        assert!(out.starts_with(FILE.split("[role").next().unwrap()));
        let parsed: Table = toml::from_str(&out).unwrap();
        assert_eq!(parsed["host"].as_array().unwrap().len(), 3);
        assert!(
            out.contains("[[host]]\nname = \"db01\"\naddress = \"10.0.0.9\"\ntags = [\"db\"]\n")
        );

        let out = edit_text(
            "",
            Kind::Check,
            None,
            Some(&fields(
                "name = \"ping\"\ninterval = \"1m\"\ncommand = \"check_ping\"",
            )),
        )
        .unwrap();
        assert_eq!(
            out,
            "[check.ping]\ncommand = \"check_ping\"\ninterval = \"1m\"\n"
        );
    }

    #[test]
    fn delete_removes_only_that_object() {
        let out = edit_text(FILE, Kind::Host, Some("web02"), None).unwrap();
        assert!(!out.contains("web02"));
        assert!(out.contains("web01"));
        assert!(out.contains("[role.linux]"));
        let out = edit_text(FILE, Kind::Contact, Some("oncall"), None).unwrap();
        assert!(!out.contains("oncall"));
        assert!(matches!(
            edit_text(FILE, Kind::Host, Some("nope"), None),
            Err(EditError::NotFound("host", _))
        ));
    }

    #[test]
    fn keyed_objects_update_and_rename() {
        let new = fields("name = \"linux\"\ninterval = \"5m\"\nmax_attempts = 3");
        let out = edit_text(FILE, Kind::Role, Some("linux"), Some(&new)).unwrap();
        assert!(out.contains("[role.linux]\ninterval = \"5m\"\nmax_attempts = 3\n"));

        let new = fields("name = \"admins\"\nemail = [\"ops@example.com\"]");
        let out = edit_text(FILE, Kind::Contact, Some("oncall"), Some(&new)).unwrap();
        assert!(out.contains("[contact.admins]\nemail = [\"ops@example.com\"]"));
        assert!(!out.contains("oncall"));

        let new = fields("name = \"linux\"");
        assert!(matches!(
            edit_text(FILE, Kind::Role, None, Some(&new)),
            Err(EditError::Exists(..))
        ));
        let out = edit_text(
            "",
            Kind::Contact,
            None,
            Some(&fields("name = \"x\"\nemail = [\"a@b\"]")),
        )
        .unwrap();
        assert_eq!(out, "[contact.x]\nemail = [\"a@b\"]\n");
    }

    #[test]
    fn nested_tables_are_merged_in_place() {
        let file = "[check.q]\ncommand = \"q.sh\"\nthresholds.depth = { warn = \"> 100\" }\n";
        let new = fields(
            "name = \"q\"\ncommand = \"q.sh\"\nthresholds.depth = { warn = \"> 100\", crit = \"> 500\" }",
        );
        let out = edit_text(file, Kind::Check, Some("q"), Some(&new)).unwrap();
        let parsed: Table = toml::from_str(&out).unwrap();
        assert_eq!(
            parsed["check"]["q"]["thresholds"]["depth"]["crit"].as_str(),
            Some("> 500")
        );
        assert!(out.starts_with("[check.q]\ncommand = \"q.sh\"\n"));
    }

    #[test]
    fn inline_definitions_are_refused() {
        let file = "host = [{ name = \"a\" }]\n";
        assert!(matches!(
            edit_text(file, Kind::Host, Some("a"), None),
            Err(EditError::Inline(..))
        ));
    }

    #[test]
    fn objects_lists_raw_fields_with_lines() {
        let found = parse("hosts.toml", FILE).unwrap();
        let names: Vec<_> = found
            .iter()
            .map(|o| (o.kind, o.name.as_str(), o.line))
            .collect();
        assert_eq!(
            names,
            [
                (Kind::Host, "web01", 2),
                (Kind::Host, "web02", 8),
                (Kind::Role, "linux", 11),
                (Kind::Contact, "oncall", 15)
            ]
        );
        assert_eq!(found[2].fields["name"].as_str(), Some("linux"));
        assert!(found[0].fields.get("use").is_none());
    }
}
