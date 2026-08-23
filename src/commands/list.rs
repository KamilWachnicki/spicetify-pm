use crate::cli::ListCommands;
use crate::errors::Result;
use crate::ledger::{Kind, Ledger};
use serde::Serialize;

#[derive(Serialize)]
struct InstalledRow {
    id: String,
    #[serde(rename = "type")]
    r#type: Kind,
    user: String,
    repo: String,
    installed_at: u64,
}

pub fn run(cmd: Option<&ListCommands>) -> Result<()> {
    match cmd {
        None => installed(None, false),
        Some(ListCommands::All { json }) => installed(None, *json),
        Some(ListCommands::Themes { json }) => installed(Some(Kind::Theme), *json),
        Some(ListCommands::Extensions { json }) => installed(Some(Kind::Extension), *json),
        Some(ListCommands::Snippets { json }) => snippets(*json),
    }
}

fn installed(kind_filter: Option<Kind>, json: bool) -> Result<()> {
    let ledger = Ledger::load()?;

    let entries: Vec<_> = ledger
        .entries
        .iter()
        .filter(|e| kind_filter.is_none_or(|k| e.kind == k))
        .collect();

    if json {
        let rows: Vec<InstalledRow> = entries
            .iter()
            .map(|e| InstalledRow {
                id: e.id.clone(),
                r#type: e.kind,
                user: e.user.clone(),
                repo: e.repo.clone(),
                installed_at: e.installed_at,
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if entries.is_empty() {
        crate::ui::info(empty_message(kind_filter));
        return Ok(());
    }
    let mut table = crate::ui::Table::new(&["Id", "Type"]);
    for entry in &entries {
        table.row(vec![
            entry.id.clone(),
            format!("{:?}", entry.kind).to_lowercase(),
        ]);
    }
    table.print();
    Ok(())
}

fn snippets(json: bool) -> Result<()> {
    let keys = crate::commands::snippets::enabled_keys(&Ledger::load()?);
    if json {
        println!("{}", serde_json::to_string_pretty(&keys)?);
        return Ok(());
    }
    if keys.is_empty() {
        crate::ui::info("no snippets enabled");
        return Ok(());
    }
    let mut table = crate::ui::Table::new(&["Key"]);
    for key in &keys {
        table.row(vec![key.clone()]);
    }
    table.print();
    Ok(())
}

fn empty_message(kind_filter: Option<Kind>) -> &'static str {
    match kind_filter {
        Some(Kind::Theme) => "no themes installed via spice-pm yet",
        Some(Kind::Extension) => "no extensions installed via spice-pm yet",
        _ => "nothing installed via spice-pm yet",
    }
}
