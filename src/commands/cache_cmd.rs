use crate::cache::{Cache, clear_dir};
use crate::errors::Result;
use crate::ui;

pub fn run_path() -> Result<()> {
    println!("{}", crate::cache::cache_dir()?.display());
    Ok(())
}

pub fn run_clear() -> Result<()> {
    let cache = Cache::new()?;
    if cache.dir().exists() {
        let dir = cache.dir().to_path_buf();
        clear_dir(&dir)?;
        ui::success(format!("cleared {}", dir.display()));
    } else {
        ui::info("cache is already empty");
    }
    Ok(())
}

pub fn run_size() -> Result<()> {
    let (count, bytes) = Cache::new()?.stats();
    println!(
        "{count} {} ({}) in {}",
        if count == 1 { "entry" } else { "entries" },
        human_bytes(bytes),
        crate::cache::cache_dir()?.display()
    );
    Ok(())
}

#[allow(clippy::cast_precision_loss)] // values here are < 1024^5, exact in f64
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
