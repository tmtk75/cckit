use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .expect("Could not find home directory")
                .join(".local/share")
        })
        .join("cckit")
}
