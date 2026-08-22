use crate::cli::ItemTypeArg;
use crate::errors::Result;
use crate::ledger::{Kind, Ledger};
use serde::Serialize;

#[derive(Serialize)]
struct InstalledRow {
    id: String,
    kind: Kind,
    user: String,
    repo: String,
    installed_at: u64,
}

pub fn run_installed(r#type: Option<ItemTypeArg>, json: bool) -> Result<()> {
    let ledger = Ledger::load()?;
    let kind_filter = r#type.map(|t| match t {
        ItemTypeArg::Extension => Kind::Extension,
        ItemTypeArg::Theme => Kind::Theme,
    });

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
                kind: e.kind,
                user: e.user.clone(),
                repo: e.repo.clone(),
                installed_at: e.installed_at,
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if entries.is_empty() {
        crate::ui::info("nothing installed via spicepm yet");
        return Ok(());
    }
    let mut table = crate::ui::Table::new(&["id", "kind"]);
    for entry in &entries {
        table.row(vec![
            entry.id.clone(),
            format!("{:?}", entry.kind).to_lowercase(),
        ]);
    }
    table.print();
    Ok(())
}
