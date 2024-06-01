
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub fn load_file(full_path: &Path) -> anyhow::Result<String> {
    println!("path {:?}", full_path);
    let mut file = File::open(full_path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    Ok(contents)
}

