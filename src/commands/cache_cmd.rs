use crate::cache::Cache;
use crate::errors::Result;
use crate::ui;

pub fn run_path() -> Result<()> {
    println!("{}", crate::cache::cache_dir()?.display());
    Ok(())
}

pub fn run_clear() -> Result<()> {
    let cache = Cache::new()?;
    if cache.dir().exists() {
        cache.clear()?;
        ui::success(format!("cleared {}", cache.dir().display()));
    } else {
        ui::info("cache is already empty");
    }
    Ok(())
}
