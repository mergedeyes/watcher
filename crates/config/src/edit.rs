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
    Service,
    Host,
    HostGroup,
    ServiceGroup,
    Contact,
    ContactGroup,
    DowntimeGroup,
    Notify,
    Sink,
}

impl Kind {
    pub const ALL: [Kind; 10] = [
        Kind::Check,
        Kind::Service,
        Kind::Host,
        Kind::HostGroup,
        Kind::ServiceGroup,
        Kind::Contact,
        Kind::ContactGroup,
        Kind::DowntimeGroup,
        Kind::Notify,
        Kind::Sink,
    ];

    /// The TOML key: `[[host]]`, `[service.x]`, ...
    pub fn key(self) -> &'static str {
        match self {
            Kind::Check => "check",
            Kind::Service => "service",
            Kind::Host => "host",
            Kind::HostGroup => "hostgroup",
            Kind::ServiceGroup => "servicegroup",
            Kind::Contact => "contact",
            Kind::ContactGroup => "contactgroup",
            Kind::DowntimeGroup => "downtimegroup",
            Kind::Notify => "notify",
            Kind::Sink => "sink",
        }
    }

    /// Named by their key (`[service.disk]`); hosts, notify rules and sinks
    /// by a `name` field in an array of tables.
    fn keyed(self) -> bool {
        !matches!(self, Kind::Host | Kind::Notify | Kind::Sink)
    }

    /// Where objects created in the UI go.
    pub fn ui_file(self) -> &'static str {
        match self {
            Kind::Check => "ui/checks.toml",
            Kind::Service => "ui/services.toml",
            Kind::Host => "ui/hosts.toml",
            Kind::HostGroup | Kind::ServiceGroup | Kind::ContactGroup | Kind::DowntimeGroup => {
                "ui/groups.toml"
            }
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
                "run_on",
                "copy",
                "passive",
                "format",
                "vars",
                "thresholds",
                "interval",
                "retry_interval",
                "max_attempts",
                "timeout",
                "freshness",
                "graphs",
            ],
            Kind::Service => &[
                "title",
                "check",
                "command",
                "shell",
                "run_on",
                "copy",
                "passive",
                "format",
                "for_each",
                "groups",
                "vars",
                "thresholds",
                "interval",
                "retry_interval",
                "max_attempts",
                "timeout",
                "freshness",
                "graphs",
                "enabled",
            ],
            Kind::Host => &[
                "name",
                "address",
                "groups",
                "services",
                "servicegroups",
                "node",
                "host_check",
                "graph",
                "ssh",
                "vars",
                "enabled",
                "service",
            ],
            Kind::HostGroup => &[
                "title",
                "members",
                "services",
                "servicegroups",
                "host_check",
                "graph",
                "ssh",
                "vars",
            ],
            Kind::ServiceGroup => &["title", "services"],
            Kind::ContactGroup => &["title", "members"],
            Kind::DowntimeGroup => &["title", "members", "hostgroups", "schedule"],
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
        (Kind::Service, file.service),
        (Kind::HostGroup, file.hostgroup),
        (Kind::ServiceGroup, file.servicegroup),
        (Kind::Contact, file.contact),
        (Kind::ContactGroup, file.contactgroup),
        (Kind::DowntimeGroup, file.downtimegroup),
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

/// One object to create (`name` is `None`), update or delete (`fields` is
/// `None`).
#[derive(Debug, Clone, PartialEq)]
pub struct Edit {
    pub kind: Kind,
    pub name: Option<String>,
    /// The object's fields as written, including `name`.
    pub fields: Option<Table>,
    /// For a new object: the file it goes to, relative to the object
    /// directory; [`Kind::ui_file`] if not set.
    pub path: Option<String>,
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
    let edits = [Edit {
        kind,
        name: name.map(String::from),
        fields: fields.cloned(),
        path: None,
    }];
    let mut changes = edit_many(dir, &edits)?;
    Ok(changes.remove(0))
}

/// Several edits at once, e.g. clones of an object: applied in order, so
/// edits of the same file build on each other. One change per file.
pub fn edit_many(dir: &Path, edits: &[Edit]) -> Result<Vec<Change>, EditError> {
    let io = |source| files::Error::Io {
        path: String::new(),
        source,
    };
    let mut found: Option<Vec<Object>> = None;
    // Path -> (hash it was read with, current text), in first-use order.
    let mut texts: Vec<(String, Option<String>, String)> = Vec::new();
    for e in edits {
        let path = match (&e.name, &e.path) {
            (None, Some(path)) => {
                files::resolve(dir, path)?;
                path.clone()
            }
            (None, None) => e.kind.ui_file().to_string(),
            (Some(name), _) => {
                if found.is_none() {
                    found = Some(objects(dir).map_err(io)?.0);
                }
                found
                    .as_ref()
                    .and_then(|list| list.iter().find(|o| o.kind == e.kind && o.name == *name))
                    .ok_or_else(|| EditError::NotFound(e.kind.key(), name.clone()))?
                    .path
                    .clone()
            }
        };
        let i = match texts.iter().position(|(p, _, _)| *p == path) {
            Some(i) => i,
            None => {
                let current = files::read(dir, &path)?;
                let (hash, text) = match current {
                    Some(f) => (Some(f.hash), f.text),
                    None => (None, String::new()),
                };
                texts.push((path.clone(), hash, text));
                texts.len() - 1
            }
        };
        let new = edit_text(&texts[i].2, e.kind, e.name.as_deref(), e.fields.as_ref()).map_err(
            |err| match err {
                EditError::Exists(k, n, _) => EditError::Exists(k, n, path.clone()),
                EditError::Inline(k, n, _) => EditError::Inline(k, n, path.clone()),
                EditError::Parse { message, .. } => EditError::Parse {
                    path: path.clone(),
                    message,
                },
                other => other,
            },
        )?;
        texts[i].2 = new;
    }
    Ok(texts
        .into_iter()
        .map(|(path, base, text)| Change {
            path,
            base,
            text: Some(text),
        })
        .collect())
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
    // Notify rules may be unnamed; everything else needs a name.
    let new_name = match fields {
        Some(f) => {
            let n = f
                .get("name")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty());
            match (n, kind) {
                (Some(n), _) => Some(n),
                (None, Kind::Notify) => None,
                (None, _) => return Err(EditError::NoName),
            }
        }
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
        // A new name must not be taken by another object in this file.
        if let (Some(new), Some(Item::ArrayOfTables(list))) = (new_name, doc.get(key))
            && name != Some(new)
            && list
                .iter()
                .any(|t| t.get("name").and_then(Item::as_str) == Some(new))
        {
            return Err(EditError::Exists(key, new.into(), String::new()));
        }
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
        table.insert(key, new_item(&fields[key], false));
    }
    table
}

/// The item for a new key. A table of tables (`service`, `thresholds`,
/// `for_each` data) becomes dotted keys with inline tables
/// (`service.http = { ... }`), or headers if its siblings are headers
/// (`[host.service.http]`); everything else an inline value.
fn new_item(value: &Value, headers: bool) -> Item {
    match value {
        Value::Table(t) if headers => {
            let mut table = toml_edit::Table::new();
            for (k, v) in t {
                table.insert(k, new_item(v, false));
            }
            Item::Table(table)
        }
        Value::Table(t) if !t.is_empty() && t.values().all(Value::is_table) => {
            let mut table = toml_edit::Table::new();
            table.set_dotted(true);
            for (k, v) in t {
                table.insert(k, Item::Value(to_edit(v)));
            }
            Item::Table(table)
        }
        v => Item::Value(to_edit(v)),
    }
}

/// Makes `table` hold exactly `fields`. Unchanged values keep their
/// formatting and comments, changed ones their comments.
fn update_table(table: &mut dyn TableLike, fields: &Table) {
    update_table_in(table, fields, None);
}

/// [`update_table`]; `header_indent` is used for new keys when no sibling
/// key shows the indentation.
fn update_table_in(table: &mut dyn TableLike, fields: &Table, header_indent: Option<String>) {
    let gone: Vec<String> = table
        .iter()
        .map(|(k, _)| k.to_string())
        .filter(|k| !fields.contains_key(k))
        .collect();
    for key in gone {
        table.remove(&key);
    }
    // New keys look like their siblings: headers among headers, and the
    // same indentation.
    let headers = table.iter().any(|(_, item)| item.is_table());
    let indent = table
        .iter()
        .filter(|(_, item)| item.is_value())
        .find_map(|(k, _)| {
            table
                .key(k)?
                .leaf_decor()
                .prefix()?
                .as_str()
                .map(String::from)
        })
        .or(header_indent);
    for (key, value) in fields {
        match table.get_mut(key) {
            Some(item) => update_item(item, value),
            None => {
                let item = new_item(value, headers && value.is_table());
                let is_value = item.is_value();
                table.insert(key, item);
                if let (true, Some(indent), Some(mut k)) = (is_value, &indent, table.key_mut(key)) {
                    k.leaf_decor_mut().set_prefix(indent.as_str());
                }
            }
        }
    }
}

fn update_item(item: &mut Item, value: &Value) {
    if let (Item::Table(table), Value::Table(fields)) = (&mut *item, value) {
        // `  [host.service.x]`: keys go below at the same indentation.
        let indent = table
            .decor()
            .prefix()
            .and_then(|p| p.as_str())
            .map(|p| p.rsplit('\n').next().unwrap_or_default().to_string())
            .filter(|p| !p.is_empty());
        update_table_in(table, fields, indent);
        return;
    }
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
groups = ["linux", "web"]

# Keep this one
[[host]]
name = "web02"

[service.load]
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
groups = ["linux", "web"]
node = "site-a""#,
        );
        let out = edit_text(FILE, Kind::Host, Some("web01"), Some(&new)).unwrap();
        assert_eq!(
            out,
            FILE.replace(
                "address = \"10.0.0.5\"   # primary\ngroups = [\"linux\", \"web\"]\n",
                "address = \"10.0.0.6\"   # primary\ngroups = [\"linux\", \"web\"]\nnode = \"site-a\"\n"
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
            r#"groups = ["db"]
address = "10.0.0.9"
name = "db01""#,
        );
        let out = edit_text(FILE, Kind::Host, None, Some(&new)).unwrap();
        assert!(out.starts_with(FILE.split("[service").next().unwrap()));
        let parsed: Table = toml::from_str(&out).unwrap();
        assert_eq!(parsed["host"].as_array().unwrap().len(), 3);
        assert!(
            out.contains("[[host]]\nname = \"db01\"\naddress = \"10.0.0.9\"\ngroups = [\"db\"]\n")
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
        assert!(out.contains("[service.load]"));
        let out = edit_text(FILE, Kind::Contact, Some("oncall"), None).unwrap();
        assert!(!out.contains("oncall"));
        assert!(matches!(
            edit_text(FILE, Kind::Host, Some("nope"), None),
            Err(EditError::NotFound("host", _))
        ));
    }

    #[test]
    fn keyed_objects_update_and_rename() {
        let new = fields("name = \"load\"\ninterval = \"5m\"\nmax_attempts = 3");
        let out = edit_text(FILE, Kind::Service, Some("load"), Some(&new)).unwrap();
        assert!(out.contains("[service.load]\ninterval = \"5m\"\nmax_attempts = 3\n"));

        let new = fields("name = \"admins\"\nemail = [\"ops@example.com\"]");
        let out = edit_text(FILE, Kind::Contact, Some("oncall"), Some(&new)).unwrap();
        assert!(out.contains("[contact.admins]\nemail = [\"ops@example.com\"]"));
        assert!(!out.contains("oncall"));

        let new = fields("name = \"load\"");
        assert!(matches!(
            edit_text(FILE, Kind::Service, None, Some(&new)),
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
    fn new_tables_of_tables_use_dotted_keys_or_match_headers() {
        let new = fields(
            "name = \"web\"\nservice.http = { check = \"http\" }\nservice.ping = {}\nvars.disks.root = { path = \"/\" }",
        );
        let out = edit_text("", Kind::Host, None, Some(&new)).unwrap();
        assert_eq!(
            out,
            "[[host]]\nname = \"web\"\nvars.disks = { root = { path = \"/\" } }\nservice.http = { check = \"http\" }\nservice.ping = {}\n"
        );

        let file =
            "[[host]]\nname = \"h\"\n  [host.service.a]\n  passive = true\n  [host.service.c]\n";
        let new = fields(
            "name = \"h\"\nservice.a = { passive = true, freshness = \"1h\" }\nservice.b = { check = \"x\" }\nservice.c = { enabled = false }",
        );
        let out = edit_text(file, Kind::Host, Some("h"), Some(&new)).unwrap();
        assert_eq!(
            out,
            "[[host]]\nname = \"h\"\n  [host.service.a]\n  passive = true\n  freshness = \"1h\"\n  [host.service.c]\n  enabled = false\n\n[host.service.b]\ncheck = \"x\"\n"
        );
    }

    #[test]
    fn many_edits_of_one_file_build_on_each_other() {
        let dir = std::env::temp_dir().join(format!("watcher-edit-many-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("hosts")).unwrap();
        std::fs::write(dir.join("hosts/web.toml"), "[[host]]\nname = \"web01\"\n").unwrap();
        let clone = |name: &str| Edit {
            kind: Kind::Host,
            name: None,
            fields: Some(fields(&format!("name = \"{name}\"\ngroups = [\"web\"]"))),
            path: Some("hosts/web.toml".into()),
        };
        let changes = edit_many(&dir, &[clone("web02"), clone("web03")]).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "hosts/web.toml");
        assert_eq!(
            changes[0].base,
            Some(crate::files::hash("[[host]]\nname = \"web01\"\n"))
        );
        let text = changes[0].text.as_deref().unwrap();
        assert!(
            text.contains("web01") && text.contains("web02") && text.contains("web03"),
            "{text}"
        );

        let bad = Edit {
            path: Some("../etc.toml".into()),
            ..clone("x")
        };
        assert!(matches!(edit_many(&dir, &[bad]), Err(EditError::File(_))));
        std::fs::remove_dir_all(&dir).unwrap();
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
                (Kind::Service, "load", 11),
                (Kind::Contact, "oncall", 15)
            ]
        );
        assert_eq!(found[2].fields["name"].as_str(), Some("load"));
        assert!(found[0].fields.get("use").is_none());
    }
}
