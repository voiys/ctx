use anyhow::Result;
use serde::Serialize;

pub(crate) fn print_toon<T: Serialize>(value: T) -> Result<()> {
    let encoded = toon_format::encode_default(&value)?;
    println!("{encoded}");
    Ok(())
}
